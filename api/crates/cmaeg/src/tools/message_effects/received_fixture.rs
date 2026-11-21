//! Retained routed user-message authority plus local native transport boundaries.
use super::{lark_fixture::App, *};
use crate::{
    discord::rest::effects_tests::Fixture as Discord,
    ingress::launch::LaunchRequest,
    net::exchange::{HttpReply, TransportError},
    telegram::TelegramTransport,
};
pub use cmaeg_core::seal::received_reaction::ReceivedReactionPlatform as Native;
use cmaeg_core::{
    activity::{ActorKind, ConversationKind, SourceKind},
    channel::{Platform, Secret},
    ids::{ConversationId, IdempotencyKey},
    message_effect::{MessageReactionRequest, TelegramReaction},
    seal::{event_ref::activity_digest, EventRef, ReplyAddress},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

pub struct Telegram {
    pub calls: Mutex<Vec<Value>>,
    pub uncertain: AtomicBool,
    pub fail_receipt: AtomicBool,
    pub delay_ms: AtomicU64,
    pub status: Mutex<StatusCode>,
    records_fail: Arc<AtomicBool>,
}
impl TelegramTransport for Telegram {
    fn post(
        &self,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        assert_eq!(url.host_str(), Some("api.telegram.org"));
        assert!(url.path().ends_with("/setMessageReaction"));
        self.calls
            .lock()
            .unwrap()
            .push(serde_json::from_slice(&body).unwrap());
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst))).await;
            if self.fail_receipt.load(Ordering::SeqCst) {
                self.records_fail.store(true, Ordering::SeqCst);
            }
            if self.uncertain.load(Ordering::SeqCst) {
                return Err(TransportError::Unavailable);
            }
            let status = *self.status.lock().unwrap();
            let body = if status == StatusCode::OK {
                json!({"ok":true,"result":true})
            } else {
                json!({"ok":false,"error_code":status.as_u16(),"parameters":{"retry_after":2}})
            };
            Ok(HttpReply::new(status, serde_json::to_vec(&body).unwrap()))
        })
    }
}
pub struct Fixture {
    pub app: App,
    pub native: Native,
    pub telegram: Arc<Telegram>,
    pub discord: Discord,
    pub default_event: Option<cmaeg_core::seal::EventRefPayload>,
}
impl Fixture {
    pub async fn new(native: Native, named: bool, threaded: bool) -> Self {
        let mut app = App::new(Platform::Lark).await;
        let discord = Discord::new().await;
        discord.memory.lock().unwrap().message = json!({"id":"800","channel_id":"99",
            "author":{"id":"42","bot":false},"type":0,"content":"original"});
        app.definition.sources = vec![serde_json::from_value(match native {
            Native::Telegram => json!({"type":"telegram","bot_id":"77","bot_username":"golden_bot","admission":{"type":"all"}}),
            Native::Discord => json!({"type":"discord","bot_id":"77","intents":37377,"admission":{"type":"all"}}),
        }).unwrap()];
        app.definition.secrets.telegram_bot_token =
            Some(Secret::new("77:received-reaction-fixture"));
        app.definition.secrets.telegram_webhook_secret =
            Some(Secret::new("received-webhook-fixture"));
        app.definition.secrets.discord_bot_token = Some(discord.token.clone());
        if named {
            app.named_consumer();
            app.definition.routing.as_mut().unwrap().default_targets = vec![
                cmaeg_core::ids::ConsumerId::default_consumer(),
                app.event.consumer.clone().unwrap(),
            ];
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
        let launch = match native {
            Native::Telegram => {
                let mut raw = json!({"update_id":101,"message":{"message_id":800,"date":now()-5,
                    "chat":{"id":42,"type":"private"},"from":{"id":42,"is_bot":false,"first_name":"Alice"},"text":"original"}});
                if threaded {
                    raw["message"]["chat"] = json!({"id":-100,"type":"supergroup"});
                    raw["message"]["is_topic_message"] = true.into();
                    raw["message"]["message_thread_id"] = 123.into();
                }
                let crate::ingress::telegram::Outcome::Admitted(observed) =
                    crate::ingress::telegram::observe(raw, "77", Some("golden_bot"), false, false)
                        .unwrap()
                else {
                    panic!("admitted human message")
                };
                let crate::ingress::media_guard::Built::Launch(launch) =
                    crate::ingress::telegram::launch(
                        channel,
                        "77",
                        *observed,
                        &app.state.keys,
                        std::time::Instant::now(),
                        now(),
                        true,
                    )
                    .unwrap()
                else {
                    panic!("ordinary message")
                };
                *launch
            }
            Native::Discord => {
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
                app.activity.occurred_at = (time::OffsetDateTime::now_utc()
                    - time::Duration::seconds(5))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap();
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
                LaunchRequest {
                    channel,
                    activity: app.activity.clone(),
                    conversation_id: app.event.conversation_id.clone(),
                    event_ref: EventRef::seal(&app.event, &app.state.keys).unwrap(),
                    idempotency_key: IdempotencyKey::from_event(&app.activity.event_id),
                    received_at: std::time::Instant::now(),
                    trace: tracing::Span::none(),
                    timing: None,
                    consumer: None,
                }
            }
        };
        let default_event = if named {
            Some(
                crate::turns::routed::bind_received_message_fixture(
                    app.state.resolver.record_client(),
                    app.state.keys.clone(),
                    &launch,
                    None,
                )
                .await
                .1,
            )
        } else {
            None
        };
        (app.activity, app.event) = crate::turns::routed::bind_received_message_fixture(
            app.state.resolver.record_client(),
            app.state.keys.clone(),
            &launch,
            selected.as_ref(),
        )
        .await;
        app.provider.memory.lock().unwrap().config.event_context = Some(json!({"schema_version":1,
            "channel_id":app.event.channel_id,"conversation_id":app.event.conversation_id,
            "event_ref":EventRef::seal(&app.event,&app.state.keys).unwrap(),"activity":app.activity}));
        let telegram = Arc::new(Telegram {
            calls: Mutex::default(),
            uncertain: AtomicBool::new(false),
            fail_receipt: AtomicBool::new(false),
            delay_ms: AtomicU64::new(0),
            status: Mutex::new(StatusCode::OK),
            records_fail: app.backend.fail_put.clone(),
        });
        let mut result = Self {
            app,
            native,
            telegram,
            discord,
            default_event,
        };
        result.gate(true);
        result
    }
    pub fn gate(&mut self, enabled: bool) {
        self.app.state.lark_effects_issuance_enabled = false;
        match self.native {
            Native::Telegram => {
                self.app.state.telegram_effects = Some(Arc::new(
                    MessageEffectsClient::new(Arc::new(
                        TelegramClient::production(Duration::from_secs(5))
                            .unwrap()
                            .with_transport(self.telegram.clone()),
                    ))
                    .with_received_reactions(enabled),
                ))
            }
            Native::Discord => {
                self.app.state.discord_effects = Some(Arc::new(
                    discord::DiscordEffectsClient::new(self.discord.api.clone())
                        .with_received_reactions(enabled),
                ))
            }
        }
    }
    pub fn writes(&self) -> usize {
        match self.native {
            Native::Telegram => self.telegram.calls.lock().unwrap().len(),
            Native::Discord => self.discord.memory.lock().unwrap().writes.len(),
        }
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
    pub async fn reference(&self) -> MessageRef {
        let received = self
            .context()
            .await
            .received_message
            .expect("received-message authority");
        assert!(received.effects.reaction && !received.effects.delete);
        assert_eq!(received.effects.scope, "received_native_user_message");
        received.message_ref
    }
}
pub fn request(emoji: &str) -> MessageReactionRequest {
    MessageReactionRequest {
        reaction: if emoji.is_empty() {
            vec![]
        } else {
            vec![TelegramReaction::Emoji {
                emoji: emoji.into(),
            }]
        },
        is_big: false,
    }
}
