use super::*;
use crate::{
    net::exchange::{BodyError, HttpReply, TransportError},
    whatsapp::{tests::credentials, Transport},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use futures_util::future::BoxFuture;
use reqwest::{StatusCode, Url};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
    time::Duration,
};

pub(crate) type Call = (Url, HeaderMap, Vec<u8>);
pub(crate) struct Graph {
    pub calls: Mutex<Vec<Call>>,
    replies: Mutex<VecDeque<Result<HttpReply, TransportError>>>,
}
impl Graph {
    pub fn new(replies: Vec<Result<HttpReply, TransportError>>) -> Arc<Self> {
        Arc::new(Self {
            calls: Mutex::default(),
            replies: Mutex::new(replies.into()),
        })
    }
}
impl Transport for Graph {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(method, Method::POST);
        assert_eq!(url.scheme(), "https");
        assert_eq!(url.host_str(), Some("graph.facebook.com"));
        assert!(headers["authorization"].is_sensitive());
        assert_eq!(
            headers["authorization"],
            format!("Bearer {}", credentials().access_token.as_str())
        );
        self.calls.lock().unwrap().push((url, headers, body));
        let reply = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected Graph call");
        Box::pin(async move { reply })
    }
}
pub(crate) fn answer(value: Value) -> Result<HttpReply, TransportError> {
    Ok(HttpReply::new(
        StatusCode::OK,
        serde_json::to_vec(&value).unwrap(),
    ))
}
pub(crate) fn confirmed(id: &str) -> Result<HttpReply, TransportError> {
    answer(
        json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":id}]}),
    )
}
pub(crate) fn attachment(mime: &str) -> NativeAttachment {
    let bytes = match mime {
        "image/png" => {
            use image::ImageEncoder;
            let mut bytes = Vec::new();
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
                .unwrap();
            bytes
        }
        "image/jpeg" => {
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut bytes)
                .encode(&[1, 2, 3], 1, 1, image::ExtendedColorType::Rgb8)
                .unwrap();
            bytes
        }
        "application/pdf" => b"%PDF-1.7\nfixture\n%%EOF".to_vec(),
        "image/webp" => sticker::tests::fixtures()[0].1.to_vec(),
        _ => audio::tests::fixtures()
            .into_iter()
            .find(|(kind, _)| *kind == mime)
            .unwrap()
            .1
            .to_vec(),
    };
    serde_json::from_value(
        json!({"kind":if mime == "image/webp" {"sticker"} else if mime.starts_with("image/") {"image"} else if mime.starts_with("audio/") {"audio"} else {"file"},
        "source":{"type":"base64","data":STANDARD.encode(bytes)},"mime_type":mime,
        "caption":if mime.starts_with("audio/") || mime == "image/webp" {None} else {Some("A caption")}}),
    )
    .unwrap()
}
async fn prepared(item: &NativeAttachment) -> Result<MediaUpload, Error> {
    let file = crate::media::outbound::prepare(std::slice::from_ref(item))
        .await
        .unwrap()
        .pop()
        .unwrap();
    Client::prepare_media(item, file).await
}

#[tokio::test]
async fn native_whatsapp_uploads_exact_bytes_then_sends_only_returned_id_to_bound_phone() {
    for mime in MIME_TYPES {
        let item = attachment(mime);
        let graph = Graph::new(vec![
            answer(json!({"id":"45678"})),
            confirmed("wamid.media"),
        ]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        let uploaded = client
            .upload_media(&credentials(), "34567", prepared(&item).await.unwrap())
            .await
            .unwrap();
        assert_eq!(
            client
                .send_media(
                    &credentials(),
                    "34567",
                    "447700900123",
                    "wamid.original",
                    uploaded
                )
                .await,
            Ok("wamid.media".into())
        );
        let calls = graph.calls.lock().unwrap();
        assert_eq!(calls.len(), 2);
        assert_eq!(calls[0].0.path(), "/v25.0/34567/media");
        assert!(calls[0].1["content-type"]
            .to_str()
            .unwrap()
            .starts_with("multipart/form-data; boundary="));
        let multipart = String::from_utf8_lossy(&calls[0].2);
        assert!(multipart.contains("name=\"messaging_product\"\r\n\r\nwhatsapp"));
        assert!(multipart.contains(&format!("Content-Type: {mime}")));
        assert!(!multipart.contains(credentials().access_token.as_str()));
        let cmaeg_core::message::NativeMediaSource::Base64 { data } = &item.source else {
            unreachable!()
        };
        let bytes = STANDARD.decode(data).unwrap();
        assert!(calls[0].2.windows(bytes.len()).any(|chunk| chunk == bytes));
        assert_eq!(calls[1].0.path(), "/v25.0/34567/messages");
        let body: Value = serde_json::from_slice(&calls[1].2).unwrap();
        let kind = if mime == "application/pdf" {
            "document"
        } else if mime == "image/webp" {
            "sticker"
        } else if mime.starts_with("audio/") {
            "audio"
        } else {
            "image"
        };
        assert_eq!(body["to"], "447700900123");
        assert_eq!(body["context"]["message_id"], "wamid.original");
        assert_eq!(body["type"], kind);
        assert_eq!(body[kind]["id"], "45678");
        if matches!(kind, "audio" | "sticker") {
            assert_eq!(body[kind], json!({"id":"45678"}));
        } else {
            assert_eq!(body[kind]["caption"], "A caption");
        }
        assert!(body[kind].get("link").is_none());
        assert_eq!(body[kind].get("filename").is_some(), kind == "document");
    }
}

#[tokio::test]
async fn native_whatsapp_audio_derives_mime_and_upload_extension_from_unlabeled_bytes() {
    for (mime, bytes) in audio::tests::fixtures() {
        let mut item = attachment(mime);
        item.mime_type = None;
        let upload = prepared(&item).await.unwrap();
        let multipart = String::from_utf8_lossy(&upload.body);
        assert!(multipart.contains(&format!("Content-Type: {mime}")));
        let extension = audio::inspect(bytes).unwrap().extensions()[0];
        assert!(multipart.contains(&format!(".{extension}\"\r\n")));
        assert_eq!(upload.kind, "audio");
        assert_eq!(upload.metadata, json!({}));
    }
}

#[tokio::test]
async fn native_whatsapp_preflight_refuses_unproved_kind_mime_filename_and_color() {
    for defect in 0..7 {
        let mut item = attachment("image/png");
        match defect {
            0 => item.kind = NativeMediaKind::Audio,
            1 => item.kind = NativeMediaKind::Video,
            2 => item.kind = NativeMediaKind::File,
            3 => item.mime_type = Some("image/jpeg".into()),
            4 => item.filename = Some("looks-like-pdf.pdf".into()),
            5 | 6 => {
                let cmaeg_core::message::NativeMediaSource::Base64 { data } = &mut item.source
                else {
                    unreachable!()
                };
                let mut bytes = STANDARD.decode(&*data).unwrap();
                bytes[if defect == 5 { 24 } else { 25 }] = if defect == 5 { 16 } else { 0 };
                *data = STANDARD.encode(bytes);
            }
            _ => unreachable!(),
        }
        assert!(
            matches!(prepared(&item).await, Err(Error::Invalid)),
            "defect {defect}"
        );
    }
    let item = attachment("image/png");
    let mut file = crate::media::outbound::prepare(std::slice::from_ref(&item))
        .await
        .unwrap()
        .pop()
        .unwrap();
    file.bytes.resize(MAX_IMAGE_BYTES + 1, 0);
    assert!(matches!(
        Client::prepare_media(&item, file).await,
        Err(Error::Invalid)
    ));
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
        .encode(&[0], 1, 1, image::ExtendedColorType::L8)
        .unwrap();
    assert!(!image_header(&jpeg, "image/jpeg"));
}

#[tokio::test]
async fn native_whatsapp_upload_and_send_uncertainty_never_retry_or_redirect() {
    for reply in [
        answer(json!({})),
        answer(json!({"id":"https://untrusted.example/file"})),
        Err(TransportError::Unavailable),
        Ok(HttpReply {
            status: StatusCode::OK,
            headers: HeaderMap::new(),
            body: Err(BodyError::Lost),
        }),
        Ok(HttpReply::new(StatusCode::FOUND, b"{}".to_vec())),
        Ok(HttpReply::new(StatusCode::BAD_GATEWAY, b"{}".to_vec())),
    ] {
        let graph = Graph::new(vec![reply]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        assert!(matches!(
            client
                .upload_media(
                    &credentials(),
                    "34567",
                    prepared(&attachment("application/pdf")).await.unwrap()
                )
                .await,
            Err(Error::Unconfirmed)
        ));
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
    for reply in [
        answer(json!({})),
        answer(
            json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"99999"}],"messages":[{"id":"wamid.wrong"}]}),
        ),
        Err(TransportError::Unavailable),
    ] {
        let graph = Graph::new(vec![answer(json!({"id":"45678"})), reply]);
        let client = Client::production().unwrap().with_transport(graph.clone());
        let uploaded = client
            .upload_media(
                &credentials(),
                "34567",
                prepared(&attachment("application/pdf")).await.unwrap(),
            )
            .await
            .unwrap();
        assert_eq!(
            client
                .send_media(
                    &credentials(),
                    "34567",
                    "447700900123",
                    "wamid.original",
                    uploaded
                )
                .await,
            Err(Error::Unconfirmed)
        );
        assert_eq!(graph.calls.lock().unwrap().len(), 2);
    }
}

#[tokio::test]
async fn native_whatsapp_upload_result_cannot_move_to_another_phone() {
    let graph = Graph::new(vec![answer(json!({"id":"45678"}))]);
    let client = Client::production().unwrap().with_transport(graph.clone());
    let uploaded = client
        .upload_media(
            &credentials(),
            "34567",
            prepared(&attachment("application/pdf")).await.unwrap(),
        )
        .await
        .unwrap();
    assert_eq!(
        client
            .send_media(
                &credentials(),
                "99999",
                "447700900123",
                "wamid.original",
                uploaded
            )
            .await,
        Err(Error::Invalid)
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}
