//! Native admitted events with retained wire evidence and entirely local transports.
use super::*;
use crate::{
    channels::definition::fixture::{replace_default_binding, replace_default_credential},
    discord::rest::effects_tests::Fixture as Discord,
    ingress::launch::LaunchRequest,
    net::exchange::{HttpReply, TransportError},
    tools::message_effects::lark_fixture::App,
    whatsapp::{tests as wa, Transport},
};
use cmaeg_core::{
    activity::{ActorKind, ConversationKind, SourceKind},
    channel::{Platform, Secret},
    ids::{ConversationId, IdempotencyKey},
    seal::{event_ref::activity_digest, EventRef},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::sync::{Arc, Mutex};

pub type Gate = (Arc<Semaphore>, Arc<Semaphore>);
pub struct Graph {
    pub calls: Mutex<Vec<(Method, String, Value)>>,
    pub response: Mutex<(StatusCode, Value)>,
    pub gate: Mutex<Option<Gate>>,
    pub delay: Mutex<Duration>,
    pub app: Mutex<String>,
    pub expected_access: Mutex<String>,
}
impl Default for Graph {
    fn default() -> Self {
        Self {
            calls: Mutex::default(),
            response: Mutex::new((StatusCode::OK, json!({"success":true}))),
            gate: Mutex::default(),
            delay: Mutex::new(Duration::ZERO),
            app: Mutex::new("12345".into()),
            expected_access: Mutex::new(wa::credentials().access_token.as_str().into()),
        }
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
        assert_eq!(url.host_str(), Some("graph.facebook.com"));
        assert!(headers["authorization"].is_sensitive());
        let expected = self.expected_access.lock().unwrap().clone();
        if url.path().ends_with("/debug_token") {
            assert!(url
                .query_pairs()
                .any(|(name, value)| name == "input_token" && value == expected));
        } else {
            assert_eq!(headers["authorization"], format!("Bearer {expected}"));
        }
        let body = if body.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&body).unwrap()
        };
        self.calls
            .lock()
            .unwrap()
            .push((method.clone(), url.path().into(), body));
        Box::pin(async move {
            let (status, body) = match (method, url.path()) {
                (Method::GET, "/v25.0/debug_token") => (
                    StatusCode::OK,
                    json!({"data":{
                    "is_valid":true,"app_id":*self.app.lock().unwrap(),
                    "scopes":["whatsapp_business_management","whatsapp_business_messaging"]}}),
                ),
                (Method::GET, "/v25.0/23456/phone_numbers") => {
                    let gate = self.gate.lock().unwrap().clone();
                    if let Some((entered, release)) = gate {
                        entered.add_permits(1);
                        release.acquire().await.unwrap().forget();
                    }
                    (StatusCode::OK, json!({"data":[{"id":"34567"}]}))
                }
                (Method::POST, "/v25.0/34567/messages") => {
                    let delay = *self.delay.lock().unwrap();
                    tokio::time::sleep(delay).await;
                    self.response.lock().unwrap().clone()
                }
                _ => panic!("unexpected native Graph operation"),
            };
            Ok(HttpReply::new(status, serde_json::to_vec(&body).unwrap()))
        })
    }
}
impl Graph {
    pub fn writes(&self) -> Vec<Value> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(method, _, _)| method == Method::POST)
            .map(|(_, _, body)| body.clone())
            .collect()
    }
    pub fn client(self: &Arc<Self>) -> crate::whatsapp::Client {
        crate::whatsapp::Client::production()
            .unwrap()
            .with_transport(self.clone())
    }
}

pub struct Fixture {
    pub app: App,
    pub discord: Discord,
    pub graph: Arc<Graph>,
}
impl Fixture {
    pub async fn whatsapp(named: bool, age: i64) -> Self {
        Self::with_wire(named, Some(age), false, |_| {}).await
    }
    pub async fn discord(named: bool, threaded: bool) -> Self {
        Self::with_wire(named, None, threaded, |_| {}).await
    }
    pub async fn with_wire(
        named: bool,
        whatsapp_age: Option<i64>,
        threaded: bool,
        change: impl FnOnce(&mut Value),
    ) -> Self {
        let mut app = App::new(Platform::Lark).await;
        let discord = Discord::new().await;
        app.definition.sources = vec![if whatsapp_age.is_some() {
            wa::source()
        } else {
            serde_json::from_value(json!({"type":"discord","bot_id":"77","intents":37377,
                "admission":{"type":"all"}}))
            .unwrap()
        }];
        app.definition.secrets.whatsapp = Some(wa::credentials());
        app.definition.secrets.discord_bot_token = Some(discord.token.clone());
        if named {
            app.named_consumer();
            app.definition.routing.as_mut().unwrap().default_targets =
                vec![app.event.consumer.clone().unwrap()];
        } else {
            app.definition.routing = Some(serde_json::from_value(json!({"version":1,
                "default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]})).unwrap());
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
        let (launch, source) = if let Some(age) = whatsapp_age {
            let mut message = wa::text("wamid.original", "hello");
            message["timestamp"] = json!((super::now() - age).to_string());
            let body = wa::envelope(vec![message]);
            let source = wa::source();
            let message = crate::whatsapp::webhook::extract(&source, &body)
                .unwrap()
                .remove(0);
            let verified = crate::ingress::whatsapp::VerifiedWhatsappMessage {
                channel,
                source,
                message,
                body: body.clone(),
                received: std::time::Instant::now(),
                received_at: super::now(),
            };
            let launch =
                crate::ingress::whatsapp::launch(&verified, None, &app.state.keys, super::now())
                    .unwrap();
            let mut body = serde_json::from_slice(&body).unwrap();
            change(&mut body);
            (launch, serde_json::to_vec(&body).unwrap())
        } else {
            app.activity.source = SourceKind::Discord {
                bot_id: "77".into(),
            };
            app.activity.event_id = "discord:77:800".parse().unwrap();
            app.activity.actor.id = "42".into();
            app.activity.actor.kind = ActorKind::Human;
            app.activity.conversation.id = "99".into();
            app.activity.conversation.kind = if threaded {
                ConversationKind::Group
            } else {
                ConversationKind::Private
            };
            app.activity.conversation.thread_id = threaded.then(|| "99".into());
            app.event.event_id = app.activity.event_id.clone();
            app.event.partition_key = app.activity.partition_key(&app.definition.partition);
            app.event.conversation_id = ConversationId::derive(
                &app.state.keys,
                &app.event.channel_id,
                app.definition.partition.version,
                &app.event.partition_key,
            );
            app.event.activity_sha256 = Some(activity_digest(&app.activity));
            app.event.reply = ReplyAddress::Discord {
                bot_id: "77".into(),
                channel_id: "99".into(),
                message_id: "800".into(),
                thread_id: app.activity.conversation.thread_id.clone(),
            };
            let mut wire = json!({"dispatch":{"id":"800","channel_id":"99",
                "author":{"id":"42","bot":false},"type":0,"content":"hello"}});
            discord.memory.lock().unwrap().message = wire["dispatch"].clone();
            change(&mut wire);
            (
                LaunchRequest {
                    channel,
                    activity: app.activity.clone(),
                    conversation_id: app.event.conversation_id.clone(),
                    event_ref: EventRef::seal(&app.event, &app.state.keys).unwrap(),
                    idempotency_key: IdempotencyKey::from_event(&app.activity.event_id),
                    received_at: std::time::Instant::now(),
                    trace: tracing::Span::none(),
                    timing: None,
                    turn_trace: None,
                    consumer: None,
                },
                serde_json::to_vec(&wire).unwrap(),
            )
        };
        (app.activity, app.event) =
            crate::turns::routed::bind_received_message_fixture_with_source(
                app.state.resolver.record_client(),
                app.state.keys.clone(),
                &launch,
                selected.as_ref(),
                &source,
            )
            .await;
        app.provider.memory.lock().unwrap().config.event_context = Some(json!({"schema_version":1,
            "channel_id":app.event.channel_id,"conversation_id":app.event.conversation_id,
            "event_ref":EventRef::seal(&app.event, &app.state.keys).unwrap(),"activity":app.activity}));
        let graph = Arc::new(Graph::default());
        app.state.native_typing = Some(Arc::new(NativeTyping::new(
            graph.client(),
            discord.api.clone(),
            true,
            true,
        )));
        Self {
            app,
            discord,
            graph,
        }
    }
    pub fn reference(&self) -> EventRef {
        EventRef::seal(&self.app.event, &self.app.state.keys).unwrap()
    }
    pub async fn typing(&self) -> Result<TypingReceipt, Problem> {
        self.app
            .state
            .typing(self.app.caller(), &self.reference())
            .await
    }
    pub async fn capability(&self) -> bool {
        self.app
            .state
            .verify_context(
                self.app.caller(),
                &self.reference(),
                self.app.activity.clone(),
            )
            .await
            .unwrap()
            .capabilities
            .typing
    }
    pub fn writes(&self) -> usize {
        self.discord.memory.lock().unwrap().writes.len() + self.graph.writes().len()
    }
    pub fn rotate(&mut self, change: usize) {
        match change {
            0 => self.app.definition.owner.subject = "other-owner".into(),
            1 => self.app.definition.sources.clear(),
            2 => replace_default_credential(
                &mut self.app.definition,
                Some("rotated-channel-fixture"),
            ),
            3 => {
                self.app
                    .definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .access_token = Secret::new("rotated-wa-fixture")
            }
            4 => {
                self.app.definition.secrets.discord_bot_token =
                    Some(Secret::new("rotated-discord-fixture"))
            }
            5 => {
                self.app.definition.secrets.agent_key_hash =
                    crate::channels::definition::hash_secret("changed-key")
            }
            6 => replace_default_binding(
                &mut self.app.definition,
                "binding-replaced".parse().unwrap(),
            ),
            _ => unreachable!(),
        }
        self.app.replace();
    }
}
