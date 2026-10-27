use super::*;
use crate::ingress::{
    launch::LaunchRequest,
    routing_plan::fixture,
    whatsapp::{self, VerifiedWhatsappMessage},
};
use crate::whatsapp::{
    media_tests::message,
    tests::{credentials, envelope, source},
    webhook,
};
use cmaeg_core::{activity::Activity, seal::EventRefPayload};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{collections::VecDeque, sync::Mutex};

pub const BYTES: &[u8] = b"native media bytes";
pub const CDN: &str =
    "https://lookaside.fbsbx.com/whatsapp_business/attachments/?mid=45678&ext=private";

#[derive(Default)]
pub struct Mock {
    pub replies: Mutex<VecDeque<(u16, Vec<u8>)>>,
    pub calls: Mutex<Vec<(String, String)>>,
}
impl Transport for Mock {
    fn get<'a>(
        &'a self,
        url: &'a Url,
        token: &'a Secret,
        _: Instant,
    ) -> BoxFuture<'a, Result<Response, FetchError>> {
        self.calls
            .lock()
            .unwrap()
            .push((url.to_string(), token.as_str().into()));
        let (status, bytes) = self
            .replies
            .lock()
            .unwrap()
            .pop_front()
            .expect("unexpected Meta call");
        Box::pin(async move {
            Ok(Response::from(
                axum::http::Response::builder()
                    .status(status)
                    .header("content-length", bytes.len())
                    .body(reqwest::Body::from(bytes))
                    .unwrap(),
            ))
        })
    }
}
impl Mock {
    pub fn bytes(&self, status: u16, bytes: &[u8]) {
        self.replies
            .lock()
            .unwrap()
            .push_back((status, bytes.into()));
    }
    pub fn meta(&self, value: Value) {
        self.bytes(200, &serde_json::to_vec(&value).unwrap());
    }
    pub fn success(&self) {
        self.meta(metadata());
        self.bytes(200, BYTES);
    }
}
pub fn metadata() -> Value {
    json!({"messaging_product":"whatsapp","id":"45678","url":CDN,"mime_type":"text/plain",
        "file_size":BYTES.len().to_string(),"sha256":webhook::hex(&Sha256::digest(BYTES))})
}
pub struct Fixture {
    pub transport: Arc<Mock>,
    pub source: WhatsappFileSource,
    pub channel: Arc<ResolvedChannel>,
    pub activity: Activity,
    pub event: EventRefPayload,
}
impl Fixture {
    pub fn new() -> Self {
        let LaunchRequest { channel, .. } = fixture::request(
            |v| {
                v["sources"] = json!([source()]);
                let c = credentials();
                v["secrets"]["whatsapp"] = json!({"app_secret":c.app_secret.as_str(),"access_token":c.access_token.as_str(),"verify_token":c.verify_token.as_str()});
            },
            fixture::message("unused"),
        );
        let body = envelope(vec![message("document", "wamid.media", BYTES)]);
        let verified = VerifiedWhatsappMessage {
            channel: channel.clone(),
            source: source(),
            message: webhook::extract(&source(), &body).unwrap().remove(0),
            body,
            received: std::time::Instant::now(),
            received_at: fixture::NOW,
        };
        let launch = whatsapp::launch(&verified, None, &fixture::keys(), fixture::NOW).unwrap();
        let event = launch
            .event_ref
            .open(&fixture::keys(), fixture::NOW)
            .unwrap()
            .payload;
        let transport = Arc::new(Mock::default());
        Self {
            source: WhatsappFileSource {
                transport: transport.clone(),
            },
            transport,
            channel,
            activity: launch.activity,
            event,
        }
    }
    pub fn request(&self) -> FetchRequest<'_> {
        let ActivityKind::Message { attachments, .. } = &self.activity.kind else {
            panic!()
        };
        FetchRequest {
            channel: &self.channel,
            event: &self.event,
            activity: &self.activity,
            locator: attachments[0].source.as_ref(),
            declared_size: None,
            credential: crate::media::source::SourceCredential::ChannelKey,
            cap: 1024,
            overflow: Overflow::Reject,
            deadline: Instant::now() + Duration::from_secs(2),
        }
    }
}
