use super::*;
use crate::delivery::intents::IntentReceipt;
use crate::{
    interactions::native_lark::fixture::Fixture,
    native_lark::{self, receipts::Store},
};
use reqwest::Method;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};

pub(crate) type Call = (Method, Url, Value);
pub(crate) struct Api {
    pub calls: Mutex<Vec<Call>>,
    pub anchor: Mutex<Value>,
    pub sent: Mutex<Value>,
    pub fail_send: AtomicBool,
    pub after_anchor: Mutex<Option<Box<dyn FnOnce() + Send>>>,
    pub after_send: Mutex<Option<Box<dyn FnOnce() + Send>>>,
}
impl Api {
    pub(crate) fn new(activity: &cmaeg_core::activity::Activity) -> Arc<Self> {
        let created = time::OffsetDateTime::parse(
            &activity.occurred_at,
            &time::format_description::well_known::Rfc3339,
        )
        .unwrap()
        .unix_timestamp_nanos()
            / 1_000_000;
        Arc::new(Self {
            calls: Mutex::default(),
            anchor: Mutex::new(
                json!({"message_id":"om_original","chat_id":activity.conversation.id,"deleted":false,"create_time":created.to_string(),"msg_type":"text","sender":{"sender_type":"user","id_type":"open_id","id":activity.actor.id,"tenant_key":"native_tenant"}}),
            ),
            sent: Mutex::new(
                json!({"message_id":"om_thread_reply","chat_id":activity.conversation.id,"thread_id":"omt_started","root_id":"om_original","parent_id":"om_original","msg_type":"text","deleted":false,"sender":{"sender_type":"app","id_type":"app_id","id":"cli_native_app","tenant_key":"native_tenant"}}),
            ),
            fail_send: AtomicBool::new(false),
            after_anchor: Mutex::default(),
            after_send: Mutex::default(),
        })
    }
    pub fn writes(&self) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, u, _)| *m == Method::POST && u.path().ends_with("/reply"))
            .count()
    }
}
impl native_lark::Transport for Api {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        self.calls.lock().unwrap().push((
            method.clone(),
            url.clone(),
            serde_json::from_slice(&body).unwrap_or(Value::Null),
        ));
        let data = if url.path().contains("tenant_access_token") {
            json!({"code":0,"tenant_access_token":"fixture-thread-token","expire":3600})
        } else if url.path().ends_with("/bot/v3/info") {
            json!({"code":0,"bot":{"open_id":"ou_native_bot"}})
        } else if method == Method::GET {
            if let Some(f) = self.after_anchor.lock().unwrap().take() {
                f();
            }
            json!({"code":0,"data":{"items":[self.anchor.lock().unwrap().clone()]}})
        } else {
            if let Some(f) = self.after_send.lock().unwrap().take() {
                f();
            }
            if self.fail_send.load(Ordering::SeqCst) {
                return Box::pin(async { Err(TransportError::Unavailable) });
            }
            json!({"code":0,"data":self.sent.lock().unwrap().clone()})
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
    sender.keys = Arc::new(crate::ingress::routing_plan::fixture::keys());
    let api = Api::new(&f.context.activity);
    sender.relay = sender
        .relay
        .with_native_lark(
            native_lark::Client::production()
                .unwrap()
                .with_transport(api.clone()),
            Arc::new(Store::new(f.server.records.clone(), sender.keys.clone())),
        )
        .with_native_lark_thread_start(true)
        .with_native_lark_effects(true)
        .with_interactions(f.store.clone());
    (sender, api)
}
pub(super) fn request() -> OutboundMessage {
    serde_json::from_value(json!({"text":"Continue in this thread", "thread_reply":true})).unwrap()
}
pub(super) async fn send(sender: &Sender, f: &Fixture, message: &OutboundMessage) -> IntentReceipt {
    let gate = RecordingGate::new(true);
    sender
        .reply(
            &SendContext {
                trace: &tracing::Span::none(),
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            },
            &f.event,
            message,
        )
        .await
        .0
}
