use super::*;
use crate::{
    ingress::{routing_plan::fixture as routed, routing_store::test_server::Memory},
    interactions::native_lark::fixture::Fixture,
    native_lark::{self, receipts::Store},
};
use reqwest::Method;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, AtomicU8, Ordering};

pub(super) type Call = (Method, Url, HeaderMap, Vec<u8>);
pub(super) struct Api {
    pub calls: Mutex<Vec<Call>>,
    pub fault: AtomicU8,
    pub upload_fault_at: AtomicU8,
    pub unique_images: AtomicBool,
    pub after_upload: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    pub memory: Arc<Memory>,
    thread: bool,
}
impl native_lark::Transport for Api {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls
            .lock()
            .unwrap()
            .push((method.clone(), url.clone(), headers, body));
        let fault = self.fault.load(Ordering::SeqCst);
        let mut row = json!({"message_id":"om_confirmed","chat_id":"oc_native_chat","msg_type":"interactive","deleted":false,"sender":{"sender_type":"app","id_type":"app_id","id":"cli_native_app","tenant_key":"native_tenant"}});
        if self.thread {
            row["thread_id"] = "omt_original".into();
        }
        let data = if url.path().contains("tenant_access_token") {
            json!({"code":0,"tenant_access_token":"fixture-image-card-access-token","expire":3600})
        } else if url.path().ends_with("/bot/v3/info") {
            let uploaded = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|(_, url, _, _)| url.path().ends_with("/images"));
            json!({"code":0,"bot":{"open_id":if fault == 6 && uploaded { "ou_replaced_bot" } else { "ou_native_bot" }}})
        } else if url.path().ends_with("/images") {
            let index = self
                .calls
                .lock()
                .unwrap()
                .iter()
                .filter(|(_, u, _, _)| u.path().ends_with("/images"))
                .count();
            let at = self.upload_fault_at.load(Ordering::SeqCst);
            let applies = at == 0 || usize::from(at) == index;
            if let Some(callback) = self.after_upload.lock().unwrap().take() {
                callback();
            }
            if fault == 1 && applies {
                json!({"code":0,"data":{"file_key":"file_wrong"}})
            } else if fault == 2 && applies {
                json!({"code":999,"msg":"rejected"})
            } else {
                let key = if self.unique_images.load(Ordering::SeqCst) {
                    format!("img_confirmed_card_{index}")
                } else {
                    "img_confirmed_card".into()
                };
                json!({"code":0,"data":{"image_key":key}})
            }
        } else if method == Method::GET {
            json!({"code":0,"data":{"items":[row]}})
        } else if method == Method::PATCH {
            json!({"code":0,"data": if fault == 5 { Value::Null } else { json!({}) }})
        } else if fault == 3 {
            json!({"code":0,"data":{"message_id":"om_uncertain","chat_id":"oc_wrong"}})
        } else {
            if fault == 4 {
                self.memory.unavailable_reads.store(10, Ordering::SeqCst);
            }
            json!({"code":0,"data":row})
        };
        Box::pin(async move {
            Ok(HttpReply::new(
                StatusCode::OK,
                serde_json::to_vec(&data).unwrap(),
            ))
        })
    }
}
pub(super) fn sender(f: &Fixture) -> (Sender, Arc<Api>) {
    let (mut sender, _, _, _) = setup();
    sender.keys = Arc::new(routed::keys());
    let api = Arc::new(Api {
        calls: Mutex::default(),
        fault: AtomicU8::new(0),
        upload_fault_at: AtomicU8::new(0),
        unique_images: AtomicBool::new(false),
        after_upload: Mutex::default(),
        memory: f.server.memory.clone(),
        thread: f.context.activity.conversation.thread_id.is_some(),
    });
    sender.relay = sender
        .relay
        .with_native_lark(
            native_lark::Client::production()
                .unwrap()
                .with_transport(api.clone()),
            Arc::new(Store::new(f.server.records.clone(), sender.keys.clone())),
        )
        .with_native_lark_controls(true, true)
        .with_native_lark_outbound_media(true)
        .with_native_lark_media_controls(true)
        .with_interactions(f.store.clone());
    (sender, api)
}
pub(super) fn message(choice: bool, mime: &str) -> OutboundMessage {
    let mut message = crate::interactions::native_lark::fixture::outbound(false);
    if choice {
        message.blocks = serde_json::from_value(json!([{"type":"choice","id":"approval","multi":false,"prompt":"Choose image","options":[{"id":"yes","label":"Yes"},{"id":"no","label":"No"}]}])).unwrap();
    }
    message.native_attachments = vec![crate::native_lark::outbound::tests::fixture::attachment(
        mime,
    )];
    message
}
pub(super) fn sent_card(calls: &[Call]) -> Value {
    let body: Value = serde_json::from_slice(
        &calls
            .iter()
            .find(|(m, u, _, _)| {
                *m == Method::POST
                    && (u.path().ends_with("/messages") || u.path().ends_with("/reply"))
            })
            .unwrap()
            .3,
    )
    .unwrap();
    assert_eq!(body["msg_type"], "interactive");
    serde_json::from_str(body["content"].as_str().unwrap()).unwrap()
}
