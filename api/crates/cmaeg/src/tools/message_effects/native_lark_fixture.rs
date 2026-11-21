//! Real sealed routing/record fixtures; native exchanges stay in memory.
use super::{lark_fixture::App, native_lark::NativeLarkEffectsClient, *};
use crate::{
    native_lark::{self, test_support as native},
    net::exchange::{HttpReply, TransportError},
};
use cmaeg_core::{
    channel::{LarkRegion, Platform, Source},
    message_effect::{MessageReactionRequest, TelegramReaction},
    seal::{EventRef, ReplyAddress},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

type Replacement = (
    crate::records::RecordClient,
    cmaeg_core::channel::ChannelDefinition,
    Arc<cmaeg_core::seal::MasterKeyRing>,
    String,
);
pub struct Api {
    pub calls: Mutex<Vec<(Method, Url, Value)>>,
    pub row: Mutex<Value>,
    pub overrides: Mutex<BTreeMap<String, Value>>,
    pub uncertain: AtomicBool,
    pub fail_receipt: AtomicBool,
    pub delay_ms: AtomicU64,
    pub replace: Mutex<Option<Replacement>>,
    pub records_fail: Arc<AtomicBool>,
}
impl native_lark::Transport for Api {
    fn exchange(
        &self,
        method: Method,
        url: Url,
        headers: HeaderMap,
        body: Vec<u8>,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        let body: Value = serde_json::from_slice(&body).unwrap_or(Value::Null);
        assert!(matches!(
            url.host_str(),
            Some("open.feishu.cn" | "open.larksuite.com")
        ));
        let auth = url.path().contains("tenant_access_token");
        if auth {
            assert!(!headers.contains_key("authorization"));
        } else {
            assert_eq!(
                headers["authorization"],
                "Bearer fixture-native-effects-token"
            );
        }
        self.calls
            .lock()
            .unwrap()
            .push((method.clone(), url.clone(), body.clone()));
        Box::pin(async move {
            let operation = format!("{method} {}", url.path());
            let mutation = method != Method::GET && !auth;
            let change = {
                let mut replacement = self.replace.lock().unwrap();
                if replacement
                    .as_ref()
                    .is_some_and(|(_, _, _, at)| at == &operation)
                {
                    replacement.take()
                } else {
                    None
                }
            };
            if let Some((records, definition, keys, _)) = change {
                let blob = cmaeg_core::seal::seal_blob(
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
            if mutation {
                tokio::time::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst)))
                    .await;
                if self.fail_receipt.load(Ordering::SeqCst) {
                    self.records_fail.store(true, Ordering::SeqCst);
                }
                if self.uncertain.load(Ordering::SeqCst) {
                    return Err(TransportError::Unavailable);
                }
            }
            let value = if let Some(value) = self.overrides.lock().unwrap().get(&operation).cloned()
            {
                value
            } else if auth {
                json!({"code":0,"tenant_access_token":"fixture-native-effects-token","expire":3600})
            } else if url.path().ends_with("/bot/v3/info") {
                json!({"code":0,"bot":{"open_id":"ou_native_bot"}})
            } else if url.path().contains("/reactions") {
                let emoji = if method == Method::POST {
                    body["reaction_type"]["emoji_type"].as_str().unwrap()
                } else {
                    "SMILE"
                };
                json!({"code":0,"data":{"reaction_id":"own_reaction","operator":{"operator_type":"app","operator_id":"cli_native_app"},"reaction_type":{"emoji_type":emoji}}})
            } else if method == Method::GET {
                json!({"code":0,"data":{"items":[self.row.lock().unwrap().clone()]}})
            } else if method == Method::PUT {
                let mut row = self.row.lock().unwrap().clone();
                row["body"] = json!({"content":body["content"]});
                json!({"code":0,"data":row})
            } else {
                json!({"code":0,"data":{}})
            };
            Ok(HttpReply::new(
                StatusCode::OK,
                serde_json::to_vec(&value).unwrap(),
            ))
        })
    }
}
pub struct Fixture {
    pub app: App,
    pub api: Arc<Api>,
}
impl Fixture {
    pub async fn new(region: LarkRegion, named: bool, threaded: bool) -> Self {
        let mut app = App::new(Platform::Lark).await;
        let Source::NativeLark(mut source) = native::source() else {
            panic!()
        };
        source.identity.region = region;
        app.definition.sources = vec![Source::NativeLark(source.clone())];
        app.definition.secrets.native_lark = Some(native::credentials());
        if named {
            app.named_consumer();
            app.definition.routing.as_mut().unwrap().default_targets = vec![
                cmaeg_core::ids::ConsumerId::default_consumer(),
                app.event.consumer.clone().unwrap(),
            ];
        } else {
            app.definition.routing = Some(serde_json::from_value(json!({"version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]})).unwrap());
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
        let message = native_lark::message::Message::Text {
            quoted_message: None,
            message_id: "om_received".into(),
            chat_id: "oc_native_chat".into(),
            sender: "ou_human".into(),
            text: "original".into(),
            resources: vec![],
            received_sticker: None,
            group: threaded,
            thread_id: threaded.then(|| "omt_thread".into()),
            mentioned: false,
            created_ms: (now() - 5) * 1000,
        };
        let launch = native_lark::message::launch(
            channel,
            &source,
            &message,
            None,
            &app.state.keys,
            std::time::Instant::now(),
            now(),
        )
        .unwrap();
        (app.activity, app.event) = crate::turns::routed::bind_received_message_fixture(
            app.state.resolver.record_client(),
            app.state.keys.clone(),
            &launch,
            selected.as_ref(),
        )
        .await;
        let row = json!({"message_id":"om_received","chat_id":"oc_native_chat","deleted":false,"msg_type":"text",
            "sender":{"sender_type":"user","id_type":"open_id","id":"ou_human","tenant_key":"native_tenant"}});
        let api = Arc::new(Api {
            calls: Mutex::default(),
            row: Mutex::new(row),
            overrides: Mutex::default(),
            uncertain: AtomicBool::new(false),
            fail_receipt: AtomicBool::new(false),
            delay_ms: AtomicU64::new(0),
            replace: Mutex::default(),
            records_fail: app.backend.fail_put.clone(),
        });
        if threaded {
            api.row.lock().unwrap()["thread_id"] = "omt_thread".into();
        }
        let mut f = Self { app, api };
        f.gates(true, true);
        f
    }
    pub fn gates(&mut self, confirmed: bool, received: bool) {
        self.app.state.lark_effects_issuance_enabled = false;
        self.app.state.lark_effects = None;
        self.app.state.native_lark_effects = Some(Arc::new(NativeLarkEffectsClient::new(
            native_lark::Client::production()
                .unwrap()
                .with_transport(self.api.clone()),
            confirmed,
            received,
        )));
    }
    pub async fn received(&self) -> MessageRef {
        self.context()
            .await
            .received_message
            .expect("received authority")
            .message_ref
    }
    pub async fn context(&self) -> crate::tools::ContextResponse {
        self.app
            .state
            .verify_context(
                self.app.caller(),
                &EventRef::seal(&self.app.event, &self.app.state.keys).unwrap(),
                self.app.activity.clone(),
            )
            .await
            .unwrap()
    }
    pub async fn confirmed(&self, editable: bool) -> MessageRef {
        let channel = self
            .app
            .state
            .resolver
            .resolve_current(self.app.event.channel_id.as_str())
            .await
            .unwrap();
        let consumer = channel.consumer(self.app.event.consumer.as_ref()).unwrap();
        let ReplyAddress::NativeLark(mut reply) = self.app.event.reply.clone() else {
            panic!()
        };
        reply.message_id = "om_sent".into();
        let receipt = native_lark::receipts::Receipt {
            kind: "native_lark_receipt_v1".into(),
            original: crate::ingress::routing_store::EventIdentity {
                owner: channel.definition.owner.subject.clone(),
                channel_id: channel.id.clone(),
                record_id: channel.pointer.record_id.clone(),
                event_id: self.app.event.event_id.clone(),
            },
            source: cmaeg_core::activity::SourceKind::NativeLark(reply.identity.clone()),
            recipient: reply.chat_id.clone(),
            reply,
            message_id: "om_sent".into(),
            consumer: consumer.id.clone(),
            provider: consumer.provider.clone(),
            partition_key: self.app.event.partition_key.clone(),
            conversation_id: self.app.event.conversation_id.clone(),
            confirmed_at: now(),
            retained_until: now() + native_lark::receipts::RETENTION,
        };
        native_lark::receipts::Store::new(
            self.app.state.resolver.record_client(),
            self.app.state.keys.clone(),
        )
        .put(&receipt)
        .await
        .unwrap();
        let mut row = self.api.row.lock().unwrap();
        row["message_id"] = "om_sent".into();
        row["sender"]["sender_type"] = "app".into();
        row["sender"]["id_type"] = "app_id".into();
        row["sender"]["id"] = "cli_native_app".into();
        drop(row);
        native_lark_authority::confirmed(&channel, &receipt, editable, &self.app.state.keys)
            .unwrap()
    }
    pub fn writes(&self) -> Vec<(Method, Url, Value)> {
        self.api
            .calls
            .lock()
            .unwrap()
            .iter()
            .filter(|(m, u, _)| *m != Method::GET && !u.path().contains("tenant_access_token"))
            .cloned()
            .collect()
    }
    pub async fn react(
        &self,
        reference: &MessageRef,
        key: &str,
        emoji: Option<&str>,
    ) -> Result<MessageEffectReceipt, cmaeg_core::problem::Problem> {
        self.app
            .state
            .set_message_reaction(
                self.app.caller(),
                reference,
                key,
                MessageReactionRequest {
                    reaction: emoji
                        .into_iter()
                        .map(|e| TelegramReaction::Emoji { emoji: e.into() })
                        .collect(),
                    is_big: false,
                },
            )
            .await
    }
    pub fn change_at(
        &self,
        at: &str,
        change: impl FnOnce(&mut cmaeg_core::channel::ChannelDefinition),
    ) {
        let mut definition = self.app.definition.clone();
        change(&mut definition);
        *self.api.replace.lock().unwrap() = Some((
            self.app.state.resolver.record_client(),
            definition,
            self.app.state.keys.clone(),
            at.into(),
        ));
    }
}
