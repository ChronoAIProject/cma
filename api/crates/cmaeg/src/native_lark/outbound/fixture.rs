use crate::{
    native_lark::{self, test_support as native},
    net::exchange::{HttpReply, TransportError},
};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{channel::NativeLarkReply, message::NativeAttachment};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    collections::VecDeque,
    sync::{Arc, Mutex},
};

pub(crate) type Call = (Url, HeaderMap, Vec<u8>);
pub(crate) struct Api {
    pub writes: Mutex<Vec<Call>>,
    pub reads: Mutex<Vec<Url>>,
    pub bot: String,
    replies: Mutex<VecDeque<Result<HttpReply, TransportError>>>,
}
impl Api {
    pub fn new(replies: Vec<Result<HttpReply, TransportError>>) -> Arc<Self> {
        Arc::new(Self {
            writes: Mutex::default(),
            reads: Mutex::default(),
            bot: native::identity().bot_open_id,
            replies: Mutex::new(replies.into()),
        })
    }
}
impl native_lark::Transport for Api {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(url.scheme(), "https");
        assert!(matches!(
            url.host_str(),
            Some("open.feishu.cn" | "open.larksuite.com")
        ));
        let reply = match url.path() {
            "/open-apis/auth/v3/tenant_access_token/internal" => {
                assert_eq!(method, Method::POST);
                assert!(!headers.contains_key("authorization"));
                let request: Value = serde_json::from_slice(&body).unwrap();
                assert_eq!(request["app_id"], native::identity().app_id);
                assert_eq!(
                    request["app_secret"],
                    native::credentials().app_secret.as_str()
                );
                self.reads.lock().unwrap().push(url);
                answer(
                    json!({"code":0,"tenant_access_token":"fixture-native-access-token","expire":7200}),
                )
            }
            "/open-apis/bot/v3/info" => {
                assert_eq!(method, Method::GET);
                self.reads.lock().unwrap().push(url);
                answer(json!({"code":0,"bot":{"open_id":self.bot}}))
            }
            _ => {
                assert_eq!(method, Method::POST);
                assert_eq!(
                    headers["authorization"],
                    "Bearer fixture-native-access-token"
                );
                assert!(headers["authorization"].is_sensitive());
                self.writes.lock().unwrap().push((url, headers, body));
                self.replies
                    .lock()
                    .unwrap()
                    .pop_front()
                    .expect("unexpected native write")
            }
        };
        Box::pin(async move { reply })
    }
}
pub(crate) fn answer(value: Value) -> Result<HttpReply, TransportError> {
    Ok(HttpReply::new(
        StatusCode::OK,
        serde_json::to_vec(&value).unwrap(),
    ))
}
pub(crate) fn uploaded(image: bool) -> Result<HttpReply, TransportError> {
    answer(if image {
        json!({"code":0,"data":{"image_key":"img_uploaded"}})
    } else {
        json!({"code":0,"data":{"file_key":"file_uploaded"}})
    })
}
pub(crate) fn confirmed(id: &str, thread: bool) -> Result<HttpReply, TransportError> {
    let mut value = json!({"code":0,"data":{"message_id":id,"chat_id":"oc_native_chat"}});
    if thread {
        value["data"]["thread_id"] = "omt_thread".into();
    }
    answer(value)
}
pub(crate) fn reply() -> NativeLarkReply {
    let cmaeg_core::channel::Source::NativeLark(source) = native::source() else {
        panic!()
    };
    NativeLarkReply {
        identity: native::identity(),
        chat_id: "oc_native_chat".into(),
        message_id: "om_original".into(),
        thread_id: None,
        revision: native_lark::revision(&source, &native::credentials()),
        mentioned: false,
    }
}
pub(crate) fn attachment(mime: &str) -> NativeAttachment {
    let (kind, bytes) = match mime {
        "image/png" => {
            use image::ImageEncoder;
            let mut bytes = Vec::new();
            image::codecs::png::PngEncoder::new(&mut bytes)
                .write_image(&[1, 2, 3, 255], 1, 1, image::ExtendedColorType::Rgba8)
                .unwrap();
            ("image", bytes)
        }
        "image/jpeg" => {
            let mut bytes = Vec::new();
            image::codecs::jpeg::JpegEncoder::new(&mut bytes)
                .encode(&[1, 2, 3], 1, 1, image::ExtendedColorType::Rgb8)
                .unwrap();
            ("image", bytes)
        }
        "application/pdf" => ("file", b"%PDF-1.7\nfixture\n%%EOF".to_vec()),
        "text/plain" => ("file", "A note in UTF-8. 你好\n".as_bytes().to_vec()),
        "audio/ogg" => (
            "audio",
            include_bytes!("../../../tests/fixtures/media/voice.ogg").to_vec(),
        ),
        "video/mp4" => (
            "video",
            include_bytes!("../../../tests/fixtures/whatsapp-outbound/whatsapp-baseline.mp4")
                .to_vec(),
        ),
        _ => panic!("unexpected fixture MIME"),
    };
    serde_json::from_value(json!({"kind":kind,"source":{"type":"base64","data":STANDARD.encode(bytes)},"mime_type":mime})).unwrap()
}
