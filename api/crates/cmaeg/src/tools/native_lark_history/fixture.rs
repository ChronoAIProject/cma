use super::*;
use crate::{
    ingress::{
        routing_plan::{self, OrdinaryPlan, RoutingOptions},
        routing_store::{Candidate, PlanStore},
    },
    native_lark::{self, test_support as native},
    net::exchange::{HttpReply, TransportError},
    tools::message_effects::lark_fixture::App,
};
use cmaeg_core::{
    channel::{ChannelDefinition, LarkRegion, Platform, Source},
    seal::seal_blob,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    collections::{BTreeMap, BTreeSet},
    sync::{
        atomic::{AtomicU64, Ordering},
        Mutex,
    },
};
type Replacement = (
    crate::records::RecordClient,
    ChannelDefinition,
    Arc<cmaeg_core::seal::MasterKeyRing>,
);
pub struct Api {
    pub calls: Mutex<Vec<(Method, Url)>>,
    pub pages: Mutex<BTreeMap<String, Value>>,
    pub anchor: Mutex<Value>,
    pub final_anchor: Mutex<Option<Value>>,
    pub replace: Mutex<Option<Replacement>>,
    pub delay: AtomicU64,
    pub list_started: tokio::sync::Notify,
    pub cancelled_lists: AtomicU64,
    pub status: Mutex<StatusCode>,
    pub bot: Mutex<String>,
}
struct InFlightList<'a> {
    cancelled: &'a AtomicU64,
    completed: bool,
}
impl Drop for InFlightList<'_> {
    fn drop(&mut self) {
        if !self.completed {
            self.cancelled.fetch_add(1, Ordering::SeqCst);
        }
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
        assert!(matches!(
            url.host_str(),
            Some("open.feishu.cn" | "open.larksuite.com")
        ));
        let auth = url.path() == "/open-apis/auth/v3/tenant_access_token/internal";
        assert_eq!(
            method,
            if auth { Method::POST } else { Method::GET },
            "no native writes"
        );
        if auth {
            assert!(!headers.contains_key("authorization"));
            assert_eq!(
                serde_json::from_slice::<Value>(&body).unwrap()["app_id"],
                "cli_native_app"
            );
        } else {
            assert_eq!(headers["authorization"], "Bearer fixture-history-token");
            assert!(body.is_empty());
        }
        self.calls.lock().unwrap().push((method, url.clone()));
        Box::pin(async move {
            let list = url.path() == "/open-apis/im/v1/messages";
            if list {
                let mut pending = InFlightList {
                    cancelled: &self.cancelled_lists,
                    completed: false,
                };
                self.list_started.notify_one();
                tokio::time::sleep(Duration::from_millis(self.delay.load(Ordering::SeqCst))).await;
                pending.completed = true;
                let change = self.replace.lock().unwrap().take();
                if let Some((records, definition, keys)) = change {
                    let blob = seal_blob(
                        &keys,
                        &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                    )
                    .unwrap();
                    records
                        .put(
                            &definition.record_id,
                            &definition.owner.subject,
                            &blob,
                            Some(1),
                        )
                        .await
                        .unwrap();
                }
            }
            let value = if auth {
                json!({"code":0,"tenant_access_token":"fixture-history-token","expire":3600})
            } else if url.path() == "/open-apis/bot/v3/info" {
                json!({"code":0,"bot":{"open_id":*self.bot.lock().unwrap()}})
            } else if list {
                let params: BTreeMap<_, _> = url.query_pairs().into_owned().collect();
                assert_eq!(params["sort_type"], "ByCreateTimeDesc");
                assert_eq!(params["page_size"], "25");
                assert_eq!(
                    params["container_id"],
                    if params["container_id_type"] == "thread" {
                        "omt_history"
                    } else {
                        "oc_history"
                    }
                );
                assert_eq!(
                    params.contains_key("end_time"),
                    params["container_id_type"] == "chat"
                );
                let value = self
                    .pages
                    .lock()
                    .unwrap()
                    .get(params.get("page_token").map(String::as_str).unwrap_or(""))
                    .expect("unexpected auto-page/token")
                    .clone();
                if let Some(anchor) = self.final_anchor.lock().unwrap().take() {
                    *self.anchor.lock().unwrap() = anchor;
                }
                value
            } else {
                assert_eq!(url.path(), "/open-apis/im/v1/messages/om_original");
                assert_eq!(url.query(), Some("user_id_type=open_id"));
                json!({"code":0,"data":{"items":[self.anchor.lock().unwrap().clone()]}})
            };
            Ok(HttpReply::new(
                if list {
                    *self.status.lock().unwrap()
                } else {
                    StatusCode::OK
                },
                serde_json::to_vec(&value).unwrap(),
            ))
        })
    }
}
pub struct Fixture {
    pub app: App,
    pub api: Arc<Api>,
    pub created: i64,
    pub threaded: bool,
}
impl Fixture {
    pub async fn new(region: LarkRegion, named: bool, threaded: bool, consent: bool) -> Self {
        Self::until(region, named, threaded, consent, now() + 600).await
    }
    pub async fn until(
        region: LarkRegion,
        named: bool,
        threaded: bool,
        consent: bool,
        expires: i64,
    ) -> Self {
        let mut app = App::new(Platform::Lark).await;
        let Source::NativeLark(mut source) = native::source() else {
            panic!()
        };
        source.identity.region = region;
        source.conversation_history = consent;
        app.definition.sources = vec![Source::NativeLark(source.clone())];
        app.definition.secrets.native_lark = Some(native::credentials());
        if named {
            app.named_consumer();
            app.definition.routing.as_mut().unwrap().default_targets =
                vec![app.event.consumer.clone().unwrap()];
        } else {
            app.definition.routing=Some(serde_json::from_value(json!({"version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]})).unwrap());
        }
        app.definition.validate().unwrap();
        app.replace();
        app.restart();
        let selected = app.event.consumer.take();
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        let created = (now() - 5) * 1000 + 456;
        let message = native_lark::message::Message::Text {
            message_id: "om_original".into(),
            chat_id: "oc_history".into(),
            sender: "ou_human".into(),
            text: "original".into(),
            resources: vec![],
            group: threaded,
            thread_id: threaded.then(|| "omt_history".into()),
            mentioned: false,
            created_ms: created,
        };
        let mut request = native_lark::message::launch(
            channel,
            &source,
            &message,
            None,
            &app.state.keys,
            std::time::Instant::now(),
            now(),
        )
        .unwrap();
        let mut event = request
            .event_ref
            .open(&app.state.keys, now())
            .unwrap()
            .payload;
        event.expires_at = expires;
        request.event_ref = EventRef::seal(&event, &app.state.keys).unwrap();
        let OrdinaryPlan::Routed(plan) = routing_plan::plan(
            &request,
            &app.state.keys,
            now(),
            RoutingOptions {
                enabled: true,
                attributes_signed: true,
                non_human_providers: &BTreeSet::new(),
            },
        )
        .unwrap() else {
            panic!()
        };
        let candidate = Candidate::prepare(
            &request,
            &plan,
            &serde_json::to_vec(&message).unwrap(),
            &app.state.keys,
            now(),
        )
        .unwrap();
        PlanStore::new(app.state.resolver.record_client(), app.state.keys.clone())
            .admit(&candidate)
            .await
            .unwrap();
        let selected = selected.unwrap_or_else(cmaeg_core::ids::ConsumerId::default_consumer);
        app.event = plan
            .targets
            .iter()
            .find(|t| t.consumer == selected)
            .unwrap()
            .event
            .clone();
        app.activity = plan.activity;
        let anchor = row("om_original", created, threaded);
        let api = Arc::new(Api {
            calls: Mutex::default(),
            pages: Mutex::new(BTreeMap::from([(String::new(), page(vec![], None))])),
            anchor: Mutex::new(anchor),
            final_anchor: Mutex::default(),
            replace: Mutex::default(),
            delay: AtomicU64::new(0),
            list_started: tokio::sync::Notify::new(),
            cancelled_lists: AtomicU64::new(0),
            status: Mutex::new(StatusCode::OK),
            bot: Mutex::new("ou_native_bot".into()),
        });
        app.state.native_lark_conversation_history =
            Some(Arc::new(NativeLarkConversationHistory::new(
                native_lark::Client::history_reader()
                    .unwrap()
                    .with_transport(api.clone()),
            )));
        Self {
            app,
            api,
            created,
            threaded,
        }
    }
    pub fn reference(&self) -> EventRef {
        EventRef::seal(&self.app.event, &self.app.state.keys).unwrap()
    }
    pub async fn read(
        &self,
        cursor: Option<&HistoryCursor>,
    ) -> Result<ConversationHistoryPage, Problem> {
        self.app
            .state
            .read_event_history(self.app.caller(), &self.reference(), cursor)
            .await
    }
    pub fn rows(&self, count: usize, next: Option<&str>) {
        self.set_page(
            "",
            (0..count)
                .map(|i| {
                    row(
                        &format!("om_row_{i}"),
                        self.created - 1000 - i as i64,
                        self.threaded,
                    )
                })
                .collect(),
            next,
        );
    }
    pub fn set_page(&self, token: &str, rows: Vec<Value>, next: Option<&str>) {
        self.api
            .pages
            .lock()
            .unwrap()
            .insert(token.into(), page(rows, next));
    }
    pub fn change_on_read(&self, change: impl FnOnce(&mut ChannelDefinition)) {
        let mut d = self.app.definition.clone();
        change(&mut d);
        *self.api.replace.lock().unwrap() = Some((
            self.app.state.resolver.record_client(),
            d,
            self.app.state.keys.clone(),
        ));
    }
    pub fn list_calls(&self) -> usize {
        self.api
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(_, url)| url.path() == "/open-apis/im/v1/messages")
            .count()
    }
}
pub fn row(id: &str, created: i64, threaded: bool) -> Value {
    let mut row = json!({"message_id":id,"chat_id":"oc_history","create_time":created.to_string(),"update_time":created.to_string(),"deleted":false,"updated":false,"msg_type":"text","sender":{"sender_type":"user","id_type":"open_id","id":"ou_human","tenant_key":"native_tenant"},"body":{"content":json!({"text":"Earlier context"}).to_string()}});
    if threaded {
        row["thread_id"] = "omt_history".into();
    }
    row
}
pub fn page(rows: Vec<Value>, next: Option<&str>) -> Value {
    json!({"code":0,"data":{"items":rows,"has_more":next.is_some(),"page_token":next}})
}
