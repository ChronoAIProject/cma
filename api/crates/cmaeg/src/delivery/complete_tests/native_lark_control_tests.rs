use super::*;
use crate::{
    ingress::routing_plan::fixture as routed,
    interactions::native_lark::fixture::{outbound, Fixture},
    native_lark::{self, receipts::Store},
};
use cmaeg_core::channel::LarkRegion;
use reqwest::Method;
use serde_json::Value;
use std::sync::atomic::{AtomicBool, Ordering};
struct Api {
    calls: Mutex<Vec<(Method, Url, Value)>>,
    thread: bool,
    bad_patch: AtomicBool,
}
impl native_lark::Transport for Api {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        let body = serde_json::from_slice::<Value>(&body).unwrap_or(Value::Null);
        self.calls
            .lock()
            .unwrap()
            .push((method.clone(), url.clone(), body));
        let mut row = json!({"message_id":"om_confirmed","chat_id":"oc_native_chat","msg_type":"interactive","deleted":false,"sender":{"sender_type":"app","id_type":"app_id","id":"cli_native_app","tenant_key":"native_tenant"}});
        if self.thread {
            row["thread_id"] = "omt_original".into();
        }
        let data = if url.path().contains("tenant_access_token") {
            json!({"code":0,"tenant_access_token":"fixture-controls-access-token","expire":3600})
        } else if url.path().ends_with("/bot/v3/info") {
            json!({"code":0,"bot":{"open_id":"ou_native_bot"}})
        } else if method == Method::GET {
            json!({"code":0,"data":{"items":[row]}})
        } else if method == Method::PATCH {
            json!({"code":0,"data": if self.bad_patch.load(Ordering::SeqCst) { Value::Null } else { json!({}) }})
        } else {
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
fn sender(f: &Fixture, controls: bool, forms: bool) -> (Sender, Arc<Api>) {
    let (mut sender, _, _, _) = setup();
    sender.keys = Arc::new(routed::keys());
    let api = Arc::new(Api {
        calls: Mutex::default(),
        thread: f.context.activity.conversation.thread_id.is_some(),
        bad_patch: AtomicBool::new(false),
    });
    sender.relay = sender
        .relay
        .with_native_lark(
            native_lark::Client::production()
                .unwrap()
                .with_transport(api.clone()),
            Arc::new(Store::new(f.server.records.clone(), sender.keys.clone())),
        )
        .with_native_lark_controls(controls, forms)
        .with_interactions(f.store.clone());
    (sender, api)
}
#[tokio::test]
async fn native_send_and_patch_bind_one_exact_message_and_preserve_thread_owner_lifetime() {
    for named in [false, true] {
        for form in [false, true] {
            let f = Fixture::new(named, true, LarkRegion::Lark).await;
            let (sender, api) = sender(&f, true, true);
            let message = outbound(form);
            let gate = RecordingGate::new(true);
            let ctx = SendContext {
                trace: &tracing::Span::none(),
                channel: &f.channel,
                first_send_done: false,
                gate: &gate,
            };
            let sent = sender.reply(&ctx, &f.event, &message).await.0;
            assert_eq!(sent.delivery.kind(), "confirmed");
            let reference = sent
                .message_ref
                .unwrap()
                .open(&sender.keys)
                .unwrap()
                .payload;
            assert!(reference.native_lark_effect.is_none());
            let (old, _) = f
                .store
                .native_lark_reference(&f.channel, &reference)
                .await
                .unwrap();
            let edited = sender.update(&ctx, &reference, &message).await.0;
            assert_eq!(edited.delivery.kind(), "confirmed");
            let updated = edited
                .message_ref
                .unwrap()
                .open(&sender.keys)
                .unwrap()
                .payload;
            assert_eq!(updated.platform_message_id, reference.platform_message_id);
            assert_eq!(updated.consumer, reference.consumer);
            let (new, _) = f
                .store
                .native_lark_reference(&f.channel, &updated)
                .await
                .unwrap();
            assert_eq!(old.expires_at, new.expires_at);
            assert!(f.store.load(&old.id).await.unwrap().0.superseded);
            assert_eq!(
                api.calls
                    .lock()
                    .unwrap()
                    .iter()
                    .filter(|(m, _, _)| *m == Method::PATCH)
                    .count(),
                1
            );
        }
    }
}
#[tokio::test]
async fn uncertain_native_patch_never_activates_replacement_or_retries_through_an_old_reference() {
    let f = Fixture::new(false, false, LarkRegion::Feishu).await;
    let card = f.card(false, true).await;
    let (sender, api) = sender(&f, true, false);
    api.bad_patch.store(true, Ordering::SeqCst);
    let reference = card
        .card
        .reference
        .unwrap()
        .open(&sender.keys)
        .unwrap()
        .payload;
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &tracing::Span::none(),
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    let first = sender.update(&ctx, &reference, &outbound(false)).await.0;
    assert_eq!(first.delivery.kind(), "unknown");
    assert!(first.message_ref.is_none());
    let retry = sender.update(&ctx, &reference, &outbound(false)).await.0;
    assert!(retry.message_ref.is_none());
    assert_eq!(
        api.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, _, _)| *m == Method::PATCH)
            .count(),
        1
    );
}
#[tokio::test]
async fn native_form_gate_is_independent_and_disabled_forms_never_create_a_message() {
    let f = Fixture::new(false, false, LarkRegion::Feishu).await;
    let (sender, api) = sender(&f, true, false);
    let gate = RecordingGate::new(true);
    let ctx = SendContext {
        trace: &tracing::Span::none(),
        channel: &f.channel,
        first_send_done: false,
        gate: &gate,
    };
    assert_eq!(
        sender
            .reply(&ctx, &f.event, &outbound(true))
            .await
            .0
            .delivery
            .kind(),
        "unsupported"
    );
    assert!(api.calls.lock().unwrap().is_empty());
}
