use super::super::fixture::App;
use super::*;
use crate::{
    net::exchange::{HttpReply, TransportError},
    telegram::TelegramTransport,
};
use cmaeg_core::{
    channel::{Secret, Source},
    seal::seal_blob,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::{json, Value};
use std::{collections::BTreeMap, sync::Mutex, time::Duration};

#[path = "rename_tests.rs"]
mod rename;

#[derive(Default)]
struct Memory {
    calls: Vec<(String, Value)>,
    overrides: BTreeMap<String, (u16, Value)>,
    hook: Option<Box<dyn FnOnce() + Send>>,
    delay: Duration,
    write_hook: Option<Box<dyn FnOnce() + Send>>,
}
#[derive(Default)]
struct Telegram(Mutex<Memory>);
impl TelegramTransport for Telegram {
    fn post<'a>(
        &'a self,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'a, Result<HttpReply, TransportError>> {
        Box::pin(async move {
            let method = url.path().rsplit('/').next().unwrap().to_owned();
            let body: Value = serde_json::from_slice(&body).unwrap();
            let (status, value, delay) = {
                let mut memory = self.0.lock().unwrap();
                memory.calls.push((method.clone(), body.clone()));
                if method == "getChatMember" {
                    if let Some(hook) = memory.hook.take() {
                        hook();
                    }
                }
                if method.ends_with("ForumTopic") {
                    if let Some(hook) = memory.write_hook.take() {
                        hook();
                    }
                }
                let fallback = match method.as_str() {
                    "getMe" => json!({"id":77,"is_bot":true,"first_name":"Fixture"}),
                    "getChat" => {
                        json!({"id":body["chat_id"].as_str().unwrap().parse::<i64>().unwrap(),"type":"supergroup","is_forum":true})
                    }
                    "getChatMember" => {
                        json!({"user":{"id":77,"is_bot":true},"status":"administrator","can_manage_topics":true})
                    }
                    "closeForumTopic" | "reopenForumTopic" | "editForumTopic" => json!(true),
                    other => panic!("unexpected native method {other}"),
                };
                let (status, value) = memory
                    .overrides
                    .get(&method)
                    .cloned()
                    .unwrap_or((200, json!({"ok":true,"result":fallback})));
                (
                    status,
                    value,
                    if method.ends_with("ForumTopic") {
                        memory.delay
                    } else {
                        Duration::ZERO
                    },
                )
            };
            tokio::time::sleep(delay).await;
            Ok(HttpReply {
                headers: HeaderMap::new(),
                status: StatusCode::from_u16(status).unwrap(),
                body: Ok(serde_json::to_vec(&value).unwrap()),
            })
        })
    }
}
impl Telegram {
    fn writes(&self) -> Vec<(String, Value)> {
        self.0
            .lock()
            .unwrap()
            .calls
            .iter()
            .filter(|(m, _)| m.ends_with("ForumTopic"))
            .cloned()
            .collect()
    }
}
async fn fixture(named: bool, chat: i64, topic: i64) -> (App, Arc<Telegram>) {
    fixture_with_consent(named, chat, topic, true).await
}
async fn fixture_with_consent(
    named: bool,
    chat: i64,
    topic: i64,
    consent: bool,
) -> (App, Arc<Telegram>) {
    let mut app = App::new(None, named).await;
    app.definition.sources = vec![serde_json::from_value(
        json!({"type":"telegram","bot_id":"77","admission":{"type":"all"},"topic_lifecycle":consent}),
    )
    .unwrap()];
    app.definition.secrets.telegram_bot_token = Some(Secret::new("77:topic-fixture"));
    app.definition.secrets.telegram_webhook_secret = Some(Secret::new("topic-webhook"));
    app.save();
    let selected = app.event.consumer.clone();
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let raw = json!({"update_id":101,"message":{"message_id":800,"date":now()-5,
        "chat":{"id":chat,"type":if chat<0 {"supergroup"} else {"private"}},"from":{"id":51,"is_bot":false,"first_name":"Alice"},
        "text":"Please finish this topic","is_topic_message":true,"message_thread_id":topic}});
    let crate::ingress::telegram::Outcome::Admitted(observed) =
        crate::ingress::telegram::observe(raw.clone(), "77", None, false, false).unwrap()
    else {
        panic!("admitted")
    };
    let crate::ingress::media_guard::Built::Launch(request) = crate::ingress::telegram::launch(
        channel,
        "77",
        *observed,
        &app.state.keys,
        std::time::Instant::now(),
        now(),
        false,
    )
    .unwrap() else {
        panic!("launch")
    };
    (app.activity, app.event) = crate::turns::routed::bind_received_message_fixture_with_source(
        app.state.resolver.record_client(),
        app.state.keys.clone(),
        &request,
        selected.as_ref(),
        &serde_json::to_vec(&raw).unwrap(),
    )
    .await;
    app.source_body = raw;
    let telegram = Arc::new(Telegram::default());
    app.state.telegram_topics = Some(Arc::new(TelegramTopics::new(Arc::new(
        TelegramClient::production(Duration::from_secs(1))
            .unwrap()
            .with_transport(telegram.clone()),
    ))));
    app.state.discord_threads = None;
    app.mount().await;
    (app, telegram)
}
async fn body(response: reqwest::Response) -> Value {
    let status = response.status();
    let value = response.json().await.unwrap();
    assert_eq!(status, 200, "{value}");
    value
}
fn caller(app: &App) -> AgentCaller {
    use sha2::{Digest, Sha256};
    AgentCaller {
        key_hash: Sha256::digest(app.key.as_bytes()).into(),
    }
}

#[tokio::test]
async fn exact_topic_rest_replay_and_truthful_discovery_for_original_consumer() {
    for named in [false, true] {
        let (app, telegram) = fixture(named, -100, 123).await;
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        let caps = app
            .state
            .topic_capabilities(caller(&app), &app.event, &channel)
            .await
            .unwrap();
        assert_eq!(
            serde_json::to_value(caps).unwrap(),
            json!({"manage_original_topic":true,"actions":["close","reopen"],"expires_at":app.event.expires_at})
        );
        assert!(app
            .state
            .thread_capabilities(caller(&app), &app.event, &channel)
            .await
            .is_none());
        let tools = crate::mcp::catalog::view(&app.state);
        assert!(tools.iter().any(|t| t.name == "manageThread"));
        assert!(!tools
            .iter()
            .any(|t| matches!(t.name, "listEventThreads" | "createEventThread")));
        for _ in 0..2 {
            assert_eq!(
                body(app.manage(app.target(), "close", "stable-close").await).await,
                json!({"outcome":{"status":"confirmed"}})
            );
        }
        assert_eq!(
            telegram.writes(),
            vec![(
                "closeForumTopic".into(),
                json!({"chat_id":"-100","message_thread_id":123})
            )]
        );
        assert_eq!(
            app.manage(app.target(), "reopen", "stable-close")
                .await
                .status(),
            409
        );
        assert_eq!(
            body(app.manage(app.target(), "reopen", "stable-reopen").await).await["outcome"]
                ["status"],
            "confirmed"
        );
        assert_eq!(telegram.writes().len(), 2);
        assert_eq!(app.create("no-create").await.status(), 403);
        assert_eq!(app.list("active").await.status(), 403);
    }
}

#[tokio::test]
async fn unsupported_actions_and_private_general_or_forged_topics_make_no_platform_call() {
    for (chat, topic) in [(51, 123), (-100, 1)] {
        let (app, telegram) = fixture(false, chat, topic).await;
        assert_eq!(
            app.manage(app.target(), "close", "unsupported")
                .await
                .status(),
            400
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty());
    }
    let (mut app, telegram) = fixture(false, -100, 123).await;
    for action in ["archive", "join", "leave"] {
        assert_eq!(
            body(app.manage(app.target(), action, action).await).await["outcome"]["status"],
            "unsupported"
        );
    }
    app.event.reply = ReplyAddress::Telegram {
        bot_id: "77".into(),
        chat_id: "-100".into(),
        message_id: "800".into(),
        thread_id: Some("124".into()),
    };
    assert!(!app
        .manage(app.target(), "close", "forged")
        .await
        .status()
        .is_success());
    assert!(telegram.0.lock().unwrap().calls.is_empty());
    let discord = App::new(Some(11), false).await;
    assert_eq!(
        body(
            discord
                .manage(discord.target(), "close", "discord-close")
                .await
        )
        .await["outcome"]["status"],
        "unsupported"
    );
    assert!(discord.memory.lock().unwrap().calls.is_empty());
}

#[tokio::test]
async fn permissions_rate_limits_and_rejected_preflight_do_not_fence_a_later_valid_write() {
    let (app, telegram) = fixture(false, -100, 123).await;
    telegram.0.lock().unwrap().overrides.insert(
        "getChatMember".into(),
        (
            200,
            json!({"ok":true,"result":{"user":{"id":77,"is_bot":true},"status":"member"}}),
        ),
    );
    assert_eq!(
        body(app.manage(app.target(), "close", "no-permission").await).await["outcome"]["status"],
        "rejected"
    );
    assert!(telegram.writes().is_empty());
    telegram.0.lock().unwrap().overrides.clear();
    telegram.0.lock().unwrap().overrides.insert(
        "closeForumTopic".into(),
        (
            429,
            json!({"ok":false,"error_code":429,"parameters":{"retry_after":7}}),
        ),
    );
    assert_eq!(
        body(app.manage(app.target(), "close", "rate-limited").await).await["outcome"],
        json!({"status":"rate_limited","retry_after":7})
    );
    telegram.0.lock().unwrap().overrides.clear();
    assert_eq!(
        body(app.manage(app.target(), "close", "now-permitted").await).await["outcome"]["status"],
        "confirmed"
    );
}

#[tokio::test]
async fn current_consent_credential_and_consumer_custody_are_rechecked_after_permissions() {
    for change in ["consent", "bot_token", "consumer_key", "provider_key"] {
        let (app, telegram) = fixture(false, -100, 123).await;
        let mut definition = app.definition.clone();
        match change {
            "consent" => {
                let Source::Telegram {
                    topic_lifecycle, ..
                } = &mut definition.sources[0]
                else {
                    panic!()
                };
                *topic_lifecycle = false;
            }
            "bot_token" => definition.secrets.telegram_bot_token = Some(Secret::new("77:rotated")),
            "consumer_key" => {
                definition.secrets.agent_key_hash =
                    crate::channels::definition::hash_secret("other-key")
            }
            "provider_key" => crate::channels::definition::fixture::replace_default_credential(
                &mut definition,
                Some("rotated-provider"),
            ),
            _ => unreachable!(),
        }
        let backend = app.backend.clone();
        let keys = app.state.keys.clone();
        telegram.0.lock().unwrap().hook = Some(Box::new(move || {
            backend.seed(
                definition.record_id.as_str(),
                &definition.owner.subject,
                seal_blob(
                    &keys,
                    &serde_json::to_vec(&definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
            );
        }));
        assert_eq!(
            body(app.manage(app.target(), "close", change).await).await["outcome"]["status"],
            "rejected"
        );
        assert!(telegram.writes().is_empty(), "{change}");
    }
}

#[tokio::test]
async fn unknown_write_survives_new_keys_restart_and_expiry_without_duplicate_effects() {
    let (mut app, telegram) = fixture(false, -100, 123).await;
    telegram
        .0
        .lock()
        .unwrap()
        .overrides
        .insert("closeForumTopic".into(), (500, json!({"ok":false})));
    let first = body(app.manage(app.target(), "close", "unknown").await).await;
    assert_eq!(first["outcome"]["status"], "unknown");
    telegram.0.lock().unwrap().overrides.clear();
    app.state.telegram_topics = Some(Arc::new(TelegramTopics::new(Arc::new(
        TelegramClient::production(Duration::from_secs(1))
            .unwrap()
            .with_transport(telegram.clone()),
    ))));
    app.mount().await;
    assert_eq!(
        body(app.manage(app.target(), "close", "unknown").await).await,
        first
    );
    assert_eq!(
        body(app.manage(app.target(), "reopen", "another-key").await).await["outcome"]["status"],
        "unknown"
    );
    assert_eq!(telegram.writes().len(), 1);
    app.backend.expire_at(app.event.expires_at + 1);
    let records = app.backend.records();
    let fences: Vec<_> = records
        .iter()
        .filter(|(id, row)| {
            cmaeg_core::seal::open_interaction(&app.state.keys, id, &row.blob)
                .ok()
                .and_then(|v| serde_json::from_slice::<Value>(&v.payload).ok())
                .is_some_and(|v| v["kind"] == "telegram_topic_fence_v1")
        })
        .collect();
    assert_eq!(fences.len(), 1);
    let store = ledger::Store::telegram(app.state.resolver.record_client(), app.state.keys.clone());
    assert!(store
        .acquire_topic("77", "-100", "123", &app.definition.owner.subject)
        .await
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn persistent_fence_includes_chat_local_topic_and_blocks_concurrent_opposite_actions() {
    let (app, telegram) = fixture(false, -100, 123).await;
    let store = ledger::Store::telegram(app.state.resolver.record_client(), app.state.keys.clone());
    let owner = &app.definition.owner.subject;
    let held = store
        .acquire_topic("77", "-100", "123", owner)
        .await
        .unwrap()
        .unwrap();
    assert!(store
        .acquire_topic("77", "-100", "123", owner)
        .await
        .unwrap()
        .is_none());
    assert!(store
        .acquire_topic("77", "-200", "123", owner)
        .await
        .unwrap()
        .is_some());
    assert_eq!(
        body(app.manage(app.target(), "reopen", "held").await).await["outcome"]["status"],
        "unknown"
    );
    assert!(telegram.0.lock().unwrap().calls.is_empty());
    store.release(held).await.unwrap();
    assert_eq!(
        body(app.manage(app.target(), "close", "free").await).await["outcome"]["status"],
        "confirmed"
    );
}

#[tokio::test]
async fn topic_mcp_close_and_reopen_share_rest_replay_and_gate() {
    let (mut app, telegram) = fixture(true, -100, 123).await;
    for action in ["close", "reopen"] {
        let response = reqwest::Client::new().post(format!("{}/mcp", app.base)).bearer_auth(&app.key)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"manageThread","arguments":{"request":{"target":app.target(),"action":action},"idempotency_key":action}}}))
            .send().await.unwrap();
        let result = body(response).await;
        assert!(result.to_string().contains("confirmed"), "{result}");
        assert_eq!(
            body(app.manage(app.target(), action, action).await).await["outcome"]["status"],
            "confirmed"
        );
    }
    assert_eq!(telegram.writes().len(), 2);
    app.state.telegram_topics = None;
    app.mount().await;
    assert!(!crate::mcp::catalog::view(&app.state)
        .iter()
        .any(|tool| tool.name == "manageThread"));
    assert_eq!(
        app.manage(app.target(), "close", "disabled").await.status(),
        403
    );
    assert_eq!(telegram.writes().len(), 2);
}

#[tokio::test]
async fn topic_deadline_or_lost_receipt_keeps_fence_across_restart() {
    for lost_receipt in [false, true] {
        let (mut app, telegram) = fixture(false, -100, 123).await;
        if lost_receipt {
            let fail = app.backend.fail_put.clone();
            telegram.0.lock().unwrap().write_hook = Some(Box::new(move || {
                fail.store(true, std::sync::atomic::Ordering::SeqCst)
            }));
        } else {
            let mut client = TelegramTopics::new(Arc::new(
                TelegramClient::production(Duration::from_secs(1))
                    .unwrap()
                    .with_transport(telegram.clone()),
            ));
            client.budget = Duration::from_millis(400);
            app.state.telegram_topics = Some(Arc::new(client));
            telegram.0.lock().unwrap().delay = Duration::from_secs(1);
            app.mount().await;
        }
        assert_eq!(
            body(app.manage(app.target(), "close", "first").await).await["outcome"]["status"],
            "unknown"
        );
        app.backend
            .fail_put
            .store(false, std::sync::atomic::Ordering::SeqCst);
        telegram.0.lock().unwrap().delay = Duration::ZERO;
        app.state.telegram_topics = Some(Arc::new(TelegramTopics::new(Arc::new(
            TelegramClient::production(Duration::from_secs(1))
                .unwrap()
                .with_transport(telegram.clone()),
        ))));
        app.mount().await;
        assert_eq!(
            body(app.manage(app.target(), "reopen", "other-key").await).await["outcome"]["status"],
            "unknown"
        );
        assert_eq!(telegram.writes().len(), 1);
    }
}

#[tokio::test]
async fn topic_missing_custody_expiry_and_optional_store_refuse_before_platform_io() {
    for defect in ["source", "owner", "consumer", "bot", "expired", "store"] {
        let (mut app, telegram) = fixture(true, -100, 123).await;
        match defect {
            "source" => {
                app.definition.sources.clear();
                app.save();
            }
            "owner" => {
                app.definition.owner.subject = "other-owner".into();
                app.save();
            }
            "consumer" => app.event.consumer = None,
            "bot" => {
                if let ReplyAddress::Telegram { bot_id, .. } = &mut app.event.reply {
                    *bot_id = "78".into();
                }
            }
            "expired" => app.event.expires_at = now() - 1,
            "store" => {
                app.state.resolver = crate::channels::resolver::ChannelResolver::new(
                    Arc::new(
                        crate::records::RecordClient::new(&app.backend.base_url, "cma-service")
                            .unwrap(),
                    ),
                    app.state.keys.clone(),
                    Default::default(),
                );
                app.mount().await;
            }
            _ => unreachable!(),
        }
        assert!(
            !app.manage(app.target(), "close", defect)
                .await
                .status()
                .is_success(),
            "{defect}"
        );
        assert!(telegram.0.lock().unwrap().calls.is_empty(), "{defect}");
        if defect == "store" {
            let channel = app
                .state
                .resolver
                .resolve_current(app.event.channel_id.as_str())
                .await
                .unwrap();
            assert!(app
                .state
                .topic_capabilities(caller(&app), &app.event, &channel)
                .await
                .is_none());
        }
    }
}

#[tokio::test]
async fn telegram_validates_names_on_non_rename_actions_before_dispatch() {
    let (app, telegram) = fixture(false, -100, 123).await;
    for action in ["close", "reopen"] {
        for name in [json!("Not allowed"), Value::Null] {
            let response = reqwest::Client::new()
                .post(format!("{}/v1/thread-operations", app.base))
                .bearer_auth(&app.key)
                .header("idempotency-key", "invalid-name")
                .json(&json!({"target":app.target(),"action":action,"name":name}))
                .send()
                .await
                .unwrap();
            assert_eq!(response.status(), 400, "{action}");
        }
    }
    assert!(telegram.0.lock().unwrap().calls.is_empty());
}
