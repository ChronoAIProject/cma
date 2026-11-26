//! All platform IO stays in process. No fixture contacts Telegram or NyxID.
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_nyxid_reply.rs"]
pub mod fake_nyxid_reply;
#[path = "fake_provider.rs"]
pub mod fake_provider;
#[path = "../../cmaeg-core/tests/routing_policy/fixture.rs"]
mod routing_fixture;
#[allow(dead_code)]
#[path = "reply_targets/support.rs"]
mod support;
use cmaeg::{
    net::exchange::{HttpReply, TransportError},
    telegram::{TelegramClient, TelegramTransport},
    tools::MessageEffectsClient,
};
use cmaeg_core::{
    channel::{AdmissionPolicy, Secret, Source},
    ids::MessageRef,
    seal::TelegramEffectAuthority,
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU8, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
use support::{receipt, App};
#[path = "message_effects/media.rs"]
mod media;

#[derive(Default)]
struct Platform {
    calls: Mutex<Vec<(String, Value)>>,
    mode: AtomicU8,
    fail_receipt: Mutex<Option<Arc<AtomicBool>>>,
}
impl TelegramTransport for Platform {
    fn post(
        &self,
        url: Url,
        _: HeaderMap,
        body: Vec<u8>,
        _: Duration,
    ) -> BoxFuture<'_, Result<HttpReply, TransportError>> {
        let method = url.path().rsplit('/').next().unwrap().to_owned();
        let body: Value = serde_json::from_slice(&body).unwrap();
        self.calls
            .lock()
            .unwrap()
            .push((method.clone(), body.clone()));
        let call_number = self.calls.lock().unwrap().len();
        Box::pin(async move {
            if method != "sendMessage" {
                if let Some(fail) = self.fail_receipt.lock().unwrap().as_ref() {
                    fail.store(true, Ordering::SeqCst);
                }
            }
            let (status, value) = match (method.as_str(), self.mode.load(Ordering::SeqCst)) {
                ("sendMessage", 5) if call_number >= 3 => return Err(TransportError::Unavailable),
                ("sendMessage" | "editMessageText", _) => (
                    StatusCode::OK,
                    json!({"ok":true,"result":{"message_id":10,"chat":{"id":body["chat_id"].as_str().unwrap().parse::<i64>().unwrap()}}}),
                ),
                (_, 1) => return Err(TransportError::Unavailable),
                (_, 2) => (
                    StatusCode::TOO_MANY_REQUESTS,
                    json!({"ok":false,"error_code":429,"parameters":{"retry_after":4}}),
                ),
                (_, 3) => (
                    StatusCode::BAD_REQUEST,
                    json!({"ok":false,"error_code":400}),
                ),
                (_, 4) => (StatusCode::OK, json!({"ok":true,"result":false})),
                _ => (StatusCode::OK, json!({"ok":true,"result":true})),
            };
            Ok(HttpReply {
                status,
                headers: HeaderMap::new(),
                body: Ok(serde_json::to_vec(&value).unwrap()),
            })
        })
    }
}
async fn setup() -> (App, Arc<Platform>, MessageRef) {
    let mut app = App::new(vec![]).await;
    app.definition.sources = vec![Source::Telegram {
        topic_lifecycle: false,
        topic_creation: false,
        message_events: false,
        channel_posts: false,
        member_join_chat_ids: Vec::new(),
        bot_id: "77".into(),
        bot_username: None,
        admission: AdmissionPolicy::All {},
    }];
    app.definition.secrets.telegram_bot_token = Some(Secret::new("77:fixture"));
    app.definition.secrets.telegram_webhook_secret =
        Some(Secret::new("native-fixture-webhook-secret"));
    app.update().await;
    let platform = Arc::new(Platform::default());
    let native = TelegramClient::production(Duration::from_secs(1))
        .unwrap()
        .with_transport(platform.clone());
    app.state.telegram_effects_issuance_enabled = true;
    app.state.telegram_effects = Some(Arc::new(MessageEffectsClient::new(Arc::new(
        native.clone(),
    ))));
    app.native = Some(native);
    app.restart().await;
    let event = app.event("42", "native-effect").await;
    let result = receipt(
        app.client
            .post(format!("{}/v1/events/{}/replies", app.base, event.as_str()))
            .bearer_auth(&app.key)
            .json(&json!({"text":"hello"}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    let reference = MessageRef::new(result["message_ref"].as_str().unwrap()).unwrap();
    assert!(reference
        .open(&app.state.keys)
        .unwrap()
        .payload
        .telegram_effect
        .is_some());
    (app, platform, reference)
}
fn effect(
    app: &App,
    reference: &MessageRef,
    operation: &str,
    key: &str,
    body: Value,
) -> reqwest::RequestBuilder {
    effect_as(app, reference, operation, key, body, &app.key)
}
fn effect_as(
    app: &App,
    reference: &MessageRef,
    operation: &str,
    key: &str,
    body: Value,
    credential: &str,
) -> reqwest::RequestBuilder {
    app.client
        .post(format!(
            "{}/v1/messages/{}/{operation}",
            app.base,
            reference.as_str()
        ))
        .bearer_auth(credential)
        .header("idempotency-key", key)
        .json(&body)
}
async fn capabilities(app: &App, reference: &MessageRef) -> Value {
    receipt(
        app.client
            .get(format!(
                "{}/v1/messages/{}/capabilities",
                app.base,
                reference.as_str()
            ))
            .bearer_auth(&app.key)
            .send()
            .await
            .unwrap(),
    )
    .await
}
#[tokio::test]
async fn confirmed_message_delete_reaction_and_clear_replay_across_restart() {
    let (mut app, platform, reference) = setup().await;
    let caps = capabilities(&app, &reference).await;
    assert_eq!(caps["delete"], true);
    assert_eq!(caps["reaction"], true);
    for (operation, key, body) in [
        (
            "reaction",
            "react",
            json!({"reaction":[{"type":"emoji","emoji":"👍"}],"is_big":true}),
        ),
        ("reaction", "clear", json!({"reaction":[]})),
        ("delete", "delete", json!({})),
    ] {
        let first = receipt(
            effect(&app, &reference, operation, key, body.clone())
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], "confirmed");
        app.restart().await;
        assert_eq!(
            receipt(
                effect(&app, &reference, operation, key, body)
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            first
        );
    }
    let calls = platform.calls.lock().unwrap();
    assert_eq!(calls.len(), 4);
    assert_eq!(calls[1].0, "setMessageReaction");
    assert_eq!(calls[1].1["chat_id"], "42");
    assert_eq!(calls[1].1["message_id"], "10");
    assert_eq!(calls[2].1["reaction"], json!([]));
    assert_eq!(calls[3].0, "deleteMessage");
}
#[tokio::test]
async fn uncertain_rate_limited_and_rejected_effects_never_resend() {
    for (mode, expected) in [
        (1, "unknown"),
        (2, "rate_limited"),
        (3, "rejected"),
        (4, "unknown"),
    ] {
        let (mut app, platform, reference) = setup().await;
        platform.mode.store(mode, Ordering::SeqCst);
        let first = receipt(
            effect(&app, &reference, "delete", "one", json!({}))
                .send()
                .await
                .unwrap(),
        )
        .await;
        assert_eq!(first["delivery"]["outcome"], expected);
        app.restart().await;
        assert_eq!(
            receipt(
                effect(&app, &reference, "delete", "one", json!({}))
                    .send()
                    .await
                    .unwrap()
            )
            .await,
            first
        );
        assert_eq!(platform.calls.lock().unwrap().len(), 2);
    }
}
#[tokio::test]
async fn concurrent_replays_and_conflicting_payload_do_not_duplicate_effects() {
    let (app, platform, reference) = setup().await;
    let (a, b) = tokio::join!(
        effect(&app, &reference, "delete", "same", json!({})).send(),
        effect(&app, &reference, "delete", "same", json!({})).send()
    );
    let a = receipt(a.unwrap()).await;
    let b = receipt(b.unwrap()).await;
    assert!(a["delivery"]["outcome"] == "confirmed" || b["delivery"]["outcome"] == "confirmed");
    assert_eq!(platform.calls.lock().unwrap().len(), 2);
    assert_eq!(
        effect(&app, &reference, "reaction", "same", json!({"reaction":[]}))
            .send()
            .await
            .unwrap()
            .status(),
        409
    );
}
#[tokio::test]
async fn live_policy_key_and_binding_revocation_refuse_cached_references() {
    for changed in ["actor", "chat", "source", "binding", "key", "consumer"] {
        let (mut app, platform, reference) = setup().await;
        app.state.resolver.resolve(app.id.as_str()).await.unwrap();
        match changed {
            "actor" => {
                if let Source::Telegram { admission, .. } = &mut app.definition.sources[0] {
                    *admission = AdmissionPolicy::Allowlist {
                        sender_ids: vec!["99".into()],
                    };
                }
            }
            "chat" => {
                if let Source::Telegram { admission, .. } = &mut app.definition.sources[0] {
                    *admission=serde_json::from_value(json!({"type":"scoped","senders":{"type":"open"},"chats":{"type":"allowlist","ids":["99"]},"groups":"all"})).unwrap();
                }
            }
            "source" => {
                if let Source::Telegram { bot_id, .. } = &mut app.definition.sources[0] {
                    *bot_id = "99".into();
                }
                app.definition.secrets.telegram_bot_token = Some(Secret::new("99:fixture"));
            }
            "binding" => app.definition.provider.binding_id = "bnd_changed".parse().unwrap(),
            "key" => app.definition.secrets.agent_key_hash = format!("sha256:{}", "ab".repeat(32)),
            "consumer" => {
                app.definition.provider.slug = "different_provider".parse().unwrap();
            }
            _ => unreachable!(),
        }
        app.update().await;
        assert!(
            !effect(&app, &reference, "delete", "denied", json!({}))
                .send()
                .await
                .unwrap()
                .status()
                .is_success(),
            "{changed}"
        );
        assert_eq!(platform.calls.lock().unwrap().len(), 1, "{changed}");
    }
}
#[tokio::test]
async fn retained_refs_and_all_relays_have_no_new_effect_authority() {
    let (app, platform, reference) = setup().await;
    let mut legacy = reference.open(&app.state.keys).unwrap().payload;
    legacy.telegram_effect = None;
    let legacy = MessageRef::seal(&legacy, &app.state.keys).unwrap();
    assert_eq!(capabilities(&app, &legacy).await["delete"], false);
    assert_eq!(
        receipt(
            effect(&app, &legacy, "delete", "legacy", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "unsupported"
    );
    assert_eq!(platform.calls.lock().unwrap().len(), 1);
}
#[tokio::test]
async fn invalid_scope_expiry_and_arbitrary_ids_are_refused_without_io() {
    let (app, platform, reference) = setup().await;
    for mutate in ["expiry", "actor", "bot", "owner", "thread"] {
        let mut payload = reference.open(&app.state.keys).unwrap().payload;
        let a: &mut TelegramEffectAuthority = payload.telegram_effect.as_mut().unwrap();
        match mutate {
            "expiry" => {
                a.issued_at = support::now() - 86401;
                a.expires_at = support::now() - 1;
            }
            "actor" => a.admission.actor_id = "invalid".into(),
            "bot" => a.bot_id = "99".into(),
            "owner" => a.owner.subject = "other-owner".into(),
            "thread" => a.thread_id = Some("not-numeric".into()),
            _ => unreachable!(),
        }
        let invalid = MessageRef::seal(&payload, &app.state.keys).unwrap();
        assert!(!effect(&app, &invalid, "delete", "invalid", json!({}))
            .send()
            .await
            .unwrap()
            .status()
            .is_success());
    }
    for body in [
        json!({"chat_id":"42","message_id":10}),
        json!({"reaction":[{"type":"paid"}]}),
        json!({"reaction":[{"type":"emoji","emoji":"👍"},{"type":"emoji","emoji":"👎"}]}),
    ] {
        assert_eq!(
            effect(&app, &reference, "reaction", "invalid-body", body)
                .send()
                .await
                .unwrap()
                .status(),
            400
        );
    }
    assert_eq!(platform.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn lost_receipt_write_preserves_unknown_and_never_replays_the_external_effect() {
    let (mut app, platform, reference) = setup().await;
    *platform.fail_receipt.lock().unwrap() = Some(app.backend.fail_put.clone());
    let first = receipt(
        effect(&app, &reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(first["delivery"]["outcome"], "unknown");
    app.backend.fail_put.store(false, Ordering::SeqCst);
    *platform.fail_receipt.lock().unwrap() = None;
    app.restart().await;
    let replay = receipt(
        effect(&app, &reference, "delete", "uncertain", json!({}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_eq!(replay["delivery"]["outcome"], "unknown");
    assert_eq!(platform.calls.lock().unwrap().len(), 2);
}

#[path = "message_effects/authority.rs"]
mod authority;

#[tokio::test]
async fn partial_native_delivery_never_issues_new_effect_authority() {
    let (app, platform, _) = setup().await;
    platform.mode.store(5, Ordering::SeqCst);
    let event = app.event("42", "partial-native-reply").await;
    let result = receipt(
        app.client
            .post(format!("{}/v1/events/{}/replies", app.base, event.as_str()))
            .bearer_auth(&app.key)
            .json(&json!({"text":"x".repeat(9000)}))
            .send()
            .await
            .unwrap(),
    )
    .await;
    assert_ne!(result["delivery"]["outcome"], "confirmed");
    let reference = MessageRef::new(result["message_ref"].as_str().unwrap()).unwrap();
    assert!(reference
        .open(&app.state.keys)
        .unwrap()
        .payload
        .telegram_effect
        .is_none());
    assert_eq!(capabilities(&app, &reference).await["delete"], false);
    assert_eq!(
        receipt(
            effect(&app, &reference, "delete", "partial", json!({}))
                .send()
                .await
                .unwrap()
        )
        .await["delivery"]["outcome"],
        "unsupported"
    );
    assert_eq!(platform.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn telegram_topic_enrollment_preserves_old_sent_effects_without_reminting_references() {
    let (mut app, platform, reference) = setup().await;
    assert!(app.definition.routing.is_none());
    app.definition.routing = Some(serde_json::from_value(json!({"version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]})).unwrap());
    for (opted, key) in [(true, "opt-in"), (false, "opt-out")] {
        let Source::Telegram {
            topic_lifecycle, ..
        } = &mut app.definition.sources[0]
        else {
            panic!()
        };
        *topic_lifecycle = opted;
        app.update().await;
        let result = receipt(
            effect(
                &app,
                &reference,
                "reaction",
                key,
                json!({"reaction":[{"type":"emoji","emoji":"👍"}]}),
            )
            .send()
            .await
            .unwrap(),
        )
        .await;
        assert_eq!(result["delivery"]["outcome"], "confirmed");
    }
    assert_eq!(platform.calls.lock().unwrap().len(), 3);
}
