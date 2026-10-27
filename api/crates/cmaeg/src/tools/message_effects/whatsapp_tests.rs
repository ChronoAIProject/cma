use super::{lark_fixture::App, *};
use crate::{
    net::exchange::{HttpReply, TransportError},
    tools::WhatsappReactionsClient,
    whatsapp::Transport,
};
use cmaeg_core::{
    activity::{ConversationKind, SourceKind},
    channel::{Platform, Secret},
    message_effect::{MessageReactionRequest, TelegramReaction},
    seal::{event_ref::activity_digest, EventRef},
};
use futures_util::future::BoxFuture;
use reqwest::{header::HeaderMap, Method, StatusCode, Url};
use serde_json::{json, Value};
use std::{
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Mutex,
    },
    time::Duration,
};

struct Graph {
    calls: Mutex<Vec<Value>>,
    uncertain: AtomicBool,
    delay_ms: AtomicU64,
    fail_receipt: AtomicBool,
    records_fail: Arc<AtomicBool>,
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
        assert_eq!(method, Method::POST);
        assert_eq!(
            url.as_str(),
            "https://graph.facebook.com/v25.0/34567/messages"
        );
        assert!(headers["authorization"].is_sensitive());
        let body: Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(body["to"], "447700900123");
        assert_eq!(body["reaction"]["message_id"], "wamid.original");
        self.calls.lock().unwrap().push(body);
        Box::pin(async move {
            tokio::time::sleep(Duration::from_millis(self.delay_ms.load(Ordering::SeqCst))).await;
            if self.fail_receipt.load(Ordering::SeqCst) {
                self.records_fail.store(true, Ordering::SeqCst);
            }
            if self.uncertain.load(Ordering::SeqCst) {
                return Err(TransportError::Unavailable);
            }
            Ok(HttpReply::new(StatusCode::OK, serde_json::to_vec(&json!({"messaging_product":"whatsapp","contacts":[{"wa_id":"447700900123"}],"messages":[{"id":"wamid.reaction"}]})).unwrap()))
        })
    }
}
fn request(emoji: &str) -> MessageReactionRequest {
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
async fn fixture() -> (App, Arc<Graph>) {
    fixture_for(false).await
}
async fn fixture_for(named: bool) -> (App, Arc<Graph>) {
    let mut app = App::new(Platform::Lark).await;
    app.definition.sources = vec![crate::whatsapp::tests::source()];
    app.definition.secrets.whatsapp = Some(crate::whatsapp::tests::credentials());
    if named {
        app.named_consumer();
        app.definition.routing.as_mut().unwrap().default_targets =
            vec![app.event.consumer.clone().unwrap()];
    } else {
        app.definition.routing = Some(serde_json::from_value(json!({
            "version":1,"default_consumer":{"name":"default"},"consumers":[],"default_targets":["default"]
        })).unwrap());
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
    // Compose signature verification, native ingress projection and real routed admission.
    // Native ingress intentionally preserves Unknown actor identity for user accounts.
    let mut text = crate::whatsapp::tests::text("wamid.original", "hello");
    text["timestamp"] = json!(now().to_string());
    let body = crate::whatsapp::tests::envelope(vec![text]);
    let secret = channel
        .definition
        .secrets
        .whatsapp
        .as_ref()
        .unwrap()
        .app_secret
        .as_str();
    let signature = ring::hmac::sign(
        &ring::hmac::Key::new(ring::hmac::HMAC_SHA256, secret.as_bytes()),
        &body,
    );
    let signature = format!(
        "sha256={}",
        crate::whatsapp::webhook::hex(signature.as_ref())
    );
    crate::whatsapp::webhook::verify(secret, &signature, &body).unwrap();
    let source = crate::whatsapp::tests::source();
    let message = crate::whatsapp::webhook::extract(&source, &body)
        .unwrap()
        .remove(0);
    let verified = crate::ingress::whatsapp::VerifiedWhatsappMessage {
        channel,
        source,
        message,
        body,
        received: std::time::Instant::now(),
        received_at: now(),
    };
    let launch = crate::ingress::whatsapp::launch(&verified, None, &app.state.keys, now()).unwrap();
    (app.activity, app.event) = crate::turns::routed::bind_received_message_fixture(
        app.state.resolver.record_client(),
        app.state.keys.clone(),
        &launch,
        selected.as_ref(),
    )
    .await;
    app.provider.memory.lock().unwrap().config.event_context = Some(json!({
        "schema_version":1,"channel_id":app.event.channel_id,"conversation_id":app.event.conversation_id,
        "event_ref":EventRef::seal(&app.event, &app.state.keys).unwrap(),"activity":app.activity,
    }));
    let graph = Arc::new(Graph {
        calls: Mutex::default(),
        uncertain: AtomicBool::new(false),
        delay_ms: AtomicU64::new(0),
        fail_receipt: AtomicBool::new(false),
        records_fail: app.backend.fail_put.clone(),
    });
    app.state.whatsapp_reactions = Some(Arc::new(WhatsappReactionsClient::new(
        crate::whatsapp::Client::production()
            .unwrap()
            .with_transport(graph.clone()),
        true,
    )));
    (app, graph)
}
async fn context(app: &App) -> crate::tools::ContextResponse {
    let reference = EventRef::seal(&app.event, &app.state.keys).unwrap();
    app.state
        .verify_context(app.caller(), &reference, app.activity.clone())
        .await
        .unwrap()
}
async fn reference(app: &App) -> MessageRef {
    let view = context(app)
        .await
        .received_message
        .expect("received-message authority");
    assert!(!view.effects.delete);
    assert!(view.effects.reaction);
    assert_eq!(view.effects.scope, "received_native_user_message");
    view.message_ref
}

#[tokio::test]
async fn native_whatsapp_reaction_context_tools_replay_replace_clear_and_edit_refusal() {
    let (app, graph) = fixture().await;
    let reference = reference(&app).await;
    let caps = app
        .state
        .message_effect_capabilities(app.caller(), &reference)
        .await
        .unwrap();
    assert!(caps.reaction && !caps.delete);
    assert_eq!(caps.scope, "received_native_user_message");
    let first = app
        .state
        .set_message_reaction(app.caller(), &reference, "first", request("👍"))
        .await
        .unwrap();
    assert_eq!(first.delivery.kind(), "confirmed");
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &reference, "first", request("👍"))
            .await
            .unwrap(),
        first
    );
    assert!(app
        .state
        .set_message_reaction(app.caller(), &reference, "first", request("❤️"))
        .await
        .is_err());
    for (key, emoji) in [("replace", "❤️"), ("clear", "")] {
        assert_eq!(
            app.state
                .set_message_reaction(app.caller(), &reference, key, request(emoji))
                .await
                .unwrap()
                .delivery
                .kind(),
            "confirmed"
        );
    }
    assert_eq!(
        graph
            .calls
            .lock()
            .unwrap()
            .iter()
            .map(|v| v["reaction"]["emoji"].clone())
            .collect::<Vec<_>>(),
        vec![json!("👍"), json!("❤️"), json!("")]
    );
    assert_eq!(
        app.state
            .delete_message(app.caller(), &reference, "delete")
            .await
            .unwrap()
            .delivery
            .kind(),
        "unsupported"
    );
    assert_eq!(
        app.state
            .update(
                app.caller(),
                &reference,
                serde_json::from_value(json!({"text":"changed"})).unwrap()
            )
            .await
            .unwrap()
            .delivery
            .kind(),
        "unsupported"
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 3);
}

#[tokio::test]
async fn native_whatsapp_reaction_source_and_credential_revocation_precede_writes() {
    for defect in 0..6 {
        let (mut app, graph) = fixture().await;
        let reference = reference(&app).await;
        match defect {
            0 => app.definition.sources.clear(),
            1 => {
                app.definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .access_token = Secret::new("replacement-access-fixture")
            }
            2 => {
                app.definition.secrets.whatsapp.as_mut().unwrap().app_secret =
                    Secret::new("replacement-app-secret-fixture")
            }
            3 => {
                app.definition
                    .secrets
                    .whatsapp
                    .as_mut()
                    .unwrap()
                    .verify_token = Secret::new("replacement-verify-fixture")
            }
            4 => app.definition.owner.subject = "different-owner".into(),
            5 => {
                if let cmaeg_core::channel::Source::Whatsapp {
                    phone_number_id, ..
                } = &mut app.definition.sources[0]
                {
                    *phone_number_id = "34568".into();
                }
            }
            _ => unreachable!(),
        }
        app.replace();
        assert!(
            app.state
                .set_message_reaction(app.caller(), &reference, "revoked", request("👍"))
                .await
                .is_err(),
            "defect {defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_reaction_retained_references_work_after_issuance_stops() {
    let (mut app, graph) = fixture().await;
    let reference = reference(&app).await;
    app.state.whatsapp_reactions = Some(Arc::new(WhatsappReactionsClient::new(
        crate::whatsapp::Client::production()
            .unwrap()
            .with_transport(graph.clone()),
        false,
    )));
    assert!(context(&app).await.received_message.is_none());
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &reference, "retained", request("👍"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_whatsapp_reaction_uncertainty_fences_new_keys_and_fresh_processes() {
    let (mut app, graph) = fixture().await;
    let original = reference(&app).await;
    graph.uncertain.store(true, Ordering::SeqCst);
    let uncertain = app
        .state
        .set_message_reaction(app.caller(), &original, "uncertain", request("👍"))
        .await
        .unwrap();
    assert_eq!(uncertain.delivery.kind(), "unknown");
    app.restart();
    graph.uncertain.store(false, Ordering::SeqCst);
    let resealed = reference(&app).await;
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &resealed, "uncertain", request("👍"))
            .await
            .unwrap(),
        uncertain
    );
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &resealed, "different", request("❤️"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_whatsapp_reaction_lost_final_receipt_never_resends() {
    let (mut app, graph) = fixture().await;
    let reference = reference(&app).await;
    graph.fail_receipt.store(true, Ordering::SeqCst);
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &reference, "lost-receipt", request("👍"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    graph.fail_receipt.store(false, Ordering::SeqCst);
    app.backend.fail_put.store(false, Ordering::SeqCst);
    app.restart();
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &reference, "replacement-key", request("👍"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "unknown"
    );
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_whatsapp_reaction_named_consumers_cannot_use_another_consumers_reference() {
    let (app, graph) = fixture_for(true).await;
    let reference = reference(&app).await;
    let default_caller = AgentCaller {
        key_hash: *app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap()
            .consumer_key_hash(None)
            .unwrap(),
    };
    assert!(app
        .state
        .set_message_reaction(default_caller, &reference, "foreign", request("👍"))
        .await
        .is_err());
    assert!(graph.calls.lock().unwrap().is_empty());
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &reference, "own", request("👍"))
            .await
            .unwrap()
            .delivery
            .kind(),
        "confirmed"
    );
}

#[tokio::test]
async fn native_whatsapp_reaction_wrong_actor_thread_source_and_stale_events_issue_no_reference() {
    for defect in 0..5 {
        let (mut app, graph) = fixture().await;
        match defect {
            0 => app.activity.actor.id = "447700900999".into(),
            1 => app.activity.conversation.kind = ConversationKind::Group,
            2 => app.activity.conversation.thread_id = Some("thread".into()),
            3 => app.activity.source = SourceKind::Custom,
            4 => {
                app.activity.occurred_at = (time::OffsetDateTime::now_utc()
                    - time::Duration::days(31))
                .format(&time::format_description::well_known::Rfc3339)
                .unwrap()
            }
            _ => unreachable!(),
        }
        app.event.activity_sha256 = Some(activity_digest(&app.activity));
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        assert!(
            whatsapp_authority::issue(&channel, &app.event, &app.activity, &app.state.keys)
                .is_none()
        );
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_reaction_concurrent_keys_send_once() {
    let (app, graph) = fixture().await;
    let reference = reference(&app).await;
    graph.delay_ms.store(75, Ordering::SeqCst);
    let first =
        app.state
            .set_message_reaction(app.caller(), &reference, "parallel-one", request("👍"));
    let second =
        app.state
            .set_message_reaction(app.caller(), &reference, "parallel-two", request("❤️"));
    let (first, second) = tokio::join!(first, second);
    let outcomes = [
        first.unwrap().delivery.kind(),
        second.unwrap().delivery.kind(),
    ];
    assert!(outcomes.contains(&"confirmed") && outcomes.contains(&"unknown"));
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_whatsapp_reaction_rotation_remint_replays_stable_receipt() {
    let (mut app, graph) = fixture().await;
    let original = reference(&app).await;
    let first = app
        .state
        .set_message_reaction(app.caller(), &original, "stable", request("👍"))
        .await
        .unwrap();
    app.definition
        .secrets
        .whatsapp
        .as_mut()
        .unwrap()
        .access_token = Secret::new("rotated-fixture-access");
    app.definition.secrets.provider_credential = Some(Secret::new("rotated-fixture-provider"));
    app.replace();
    app.restart();
    assert!(app
        .state
        .set_message_reaction(app.caller(), &original, "stale", request("👍"))
        .await
        .is_err());
    let renewed = reference(&app).await;
    assert_ne!(original, renewed);
    assert_eq!(
        app.state
            .set_message_reaction(app.caller(), &renewed, "stable", request("👍"))
            .await
            .unwrap(),
        first
    );
    let conflict = app
        .state
        .set_message_reaction(app.caller(), &renewed, "stable", request("❤️"))
        .await
        .unwrap_err();
    assert_eq!(conflict.code, cmaeg_core::problem::ProblemCode::Conflict);
    assert_eq!(graph.calls.lock().unwrap().len(), 1);
}

#[tokio::test]
async fn native_whatsapp_reaction_changed_original_source_or_provider_cannot_remint() {
    for defect in 0..3 {
        let (mut app, graph) = fixture().await;
        reference(&app).await;
        match defect {
            0 => {
                let cmaeg_core::channel::Source::Whatsapp { admission, .. } =
                    &mut app.definition.sources[0]
                else {
                    panic!()
                };
                *admission = cmaeg_core::channel::AdmissionPolicy::Allowlist {
                    sender_ids: vec!["447700900123".into()],
                };
            }
            1 => app.definition.provider.binding_id = "replacement-provider".parse().unwrap(),
            2 => app.definition.secrets.provider_credential = None,
            _ => unreachable!(),
        }
        app.replace();
        app.restart();
        assert!(
            context(&app).await.received_message.is_none(),
            "defect {defect}"
        );
        assert!(graph.calls.lock().unwrap().is_empty());
    }
}

#[tokio::test]
async fn native_whatsapp_reaction_both_context_paths_require_retained_binding_and_preserve_context()
{
    let (mut app, graph) = fixture().await;
    let original = EventRef::seal(&app.event, &app.state.keys).unwrap();
    assert!(app
        .state
        .context(app.caller(), &original)
        .await
        .unwrap()
        .received_message
        .is_some());
    app.event.partition_key.push_str(":unbound");
    let unbound = EventRef::seal(&app.event, &app.state.keys).unwrap();
    for view in [
        app.state.context(app.caller(), &unbound).await.unwrap(),
        app.state
            .verify_context(app.caller(), &unbound, app.activity.clone())
            .await
            .unwrap(),
    ] {
        assert!(view.received_message.is_none());
        assert!(view.activity_verified);
        assert!(!serde_json::to_value(view)
            .unwrap()
            .as_object()
            .unwrap()
            .contains_key("received_message"));
    }
    assert!(graph.calls.lock().unwrap().is_empty());
}

#[tokio::test]
async fn native_whatsapp_reaction_short_reference_does_not_shorten_receipt_or_uncertainty_ttl() {
    for uncertain in [false, true] {
        let (mut app, graph) = fixture().await;
        let original = reference(&app).await;
        let payload = original.open(&app.state.keys).unwrap().payload;
        let authority = payload.whatsapp_reaction.as_ref().unwrap();
        let full_expiry =
            authority.received_at + cmaeg_core::seal::whatsapp_reaction::MAX_REACTION_AGE;
        assert!(authority.expires_at < full_expiry);
        graph.uncertain.store(uncertain, Ordering::SeqCst);
        let first = app
            .state
            .set_message_reaction(app.caller(), &original, "retained", request("👍"))
            .await
            .unwrap();
        let retained = app
            .backend
            .records()
            .into_iter()
            .filter_map(|(id, row)| {
                let opened =
                    cmaeg_core::seal::open_interaction(&app.state.keys, &id, &row.blob).ok()?;
                serde_json::from_slice::<Value>(&opened.payload).ok()
            })
            .collect::<Vec<_>>();
        for kind in [
            "whatsapp_received_reaction_v1",
            "whatsapp_received_mutation_v1",
        ] {
            let record = retained.iter().find(|row| row["kind"] == kind).unwrap();
            assert_eq!(record["expires_at"], full_expiry);
        }
        // A second configured channel receives the same physical user message.
        // Its independently admitted context expires after the first context.
        let first_expiry = app.event.expires_at;
        app.definition.record_id = uuid::Uuid::new_v4().to_string().parse().unwrap();
        app.event.record_id = app.definition.record_id.clone();
        let pointer = cmaeg_core::seal::ChannelPointer {
            v: 1,
            record_id: app.definition.record_id.clone(),
            record_url: app
                .state
                .resolver
                .record_client()
                .record_url(app.definition.record_id.as_str())
                .unwrap()
                .to_string(),
        };
        app.event.channel_id = pointer.seal_id(&app.state.keys).unwrap();
        app.event.partition_key = app.activity.partition_key(&app.definition.partition);
        app.event.conversation_id = cmaeg_core::ids::ConversationId::derive(
            &app.state.keys,
            &app.event.channel_id,
            app.definition.partition.version,
            &app.event.partition_key,
        );
        app.event.expires_at = first_expiry + 60;
        app.activity.routing = None;
        app.event.activity_sha256 = Some(activity_digest(&app.activity));
        app.replace();
        let channel = app
            .state
            .resolver
            .resolve_current(app.event.channel_id.as_str())
            .await
            .unwrap();
        let launch = crate::ingress::launch::LaunchRequest {
            channel,
            activity: app.activity.clone(),
            conversation_id: app.event.conversation_id.clone(),
            event_ref: EventRef::seal(&app.event, &app.state.keys).unwrap(),
            idempotency_key: cmaeg_core::ids::IdempotencyKey::from_event(&app.activity.event_id),
            received_at: std::time::Instant::now(),
            trace: tracing::Span::none(),
            timing: None,
            consumer: None,
        };
        (app.activity, app.event) = crate::turns::routed::bind_received_message_fixture(
            app.state.resolver.record_client(),
            app.state.keys.clone(),
            &launch,
            None,
        )
        .await;
        let later = reference(&app).await;
        assert!(
            later
                .open(&app.state.keys)
                .unwrap()
                .payload
                .whatsapp_reaction
                .unwrap()
                .expires_at
                > first_expiry
        );
        app.backend.expire_at(first_expiry + 1);
        app.restart();
        graph.uncertain.store(false, Ordering::SeqCst);
        assert_eq!(
            app.state
                .set_message_reaction(app.caller(), &original, "retained", request("👍"))
                .await
                .unwrap(),
            first
        );
        if uncertain {
            assert_eq!(
                app.state
                    .set_message_reaction(app.caller(), &later, "another", request("❤️"))
                    .await
                    .unwrap()
                    .delivery
                    .kind(),
                "unknown"
            );
        }
        assert_eq!(graph.calls.lock().unwrap().len(), 1);
    }
}

#[tokio::test]
async fn native_whatsapp_reaction_source_age_and_event_expiry_are_independent_bounds() {
    let (mut app, _) = fixture().await;
    let channel = app
        .state
        .resolver
        .resolve_current(app.event.channel_id.as_str())
        .await
        .unwrap();
    let current = now();
    for (age, event_expiry, expected) in [
        (30 * 86400 + 1, current + 600, false),
        (30 * 86400, current + 600, false),
        (30 * 86400 - 120, current + 600, true),
        (60, current, false),
        (-60, current + 600, false),
    ] {
        app.activity.occurred_at = time::OffsetDateTime::from_unix_timestamp(current - age)
            .unwrap()
            .format(&time::format_description::well_known::Rfc3339)
            .unwrap();
        app.event.activity_sha256 = Some(activity_digest(&app.activity));
        app.event.expires_at = event_expiry;
        let issued =
            whatsapp_authority::issue(&channel, &app.event, &app.activity, &app.state.keys);
        assert_eq!(
            issued.is_some(),
            expected,
            "age {age}, expiry {event_expiry}"
        );
        if let Some(issued) = issued {
            assert_eq!(
                issued
                    .open(&app.state.keys)
                    .unwrap()
                    .payload
                    .whatsapp_reaction
                    .unwrap()
                    .expires_at,
                current + 120
            );
        }
    }
}

#[tokio::test]
async fn native_whatsapp_reaction_discovery_uses_its_own_issuance_flag() {
    let (mut app, graph) = fixture().await;
    app.state.lark_effects_issuance_enabled = false;
    let tools = crate::mcp::catalog::view(&app.state);
    let effect = tools
        .iter()
        .find(|tool| tool.name == "setMessageReaction")
        .unwrap();
    assert!(effect
        .description
        .contains("received native WhatsApp user message"));
    assert!(tools
        .iter()
        .any(|tool| tool.name == "readMessageCapabilities"));
    app.state.whatsapp_reactions = Some(Arc::new(WhatsappReactionsClient::new(
        crate::whatsapp::Client::production()
            .unwrap()
            .with_transport(graph),
        false,
    )));
    assert!(!crate::mcp::catalog::view(&app.state)
        .iter()
        .any(|tool| tool.name == "setMessageReaction"));
}

#[tokio::test]
async fn native_whatsapp_reaction_metadata_timeout_preserves_verified_context() {
    let (app, graph) = fixture().await;
    reference(&app).await; // Warm ordinary event authorization before delaying metadata reads.
    app.backend.get_delay_ms.store(2500, Ordering::SeqCst);
    let started = std::time::Instant::now();
    let view = context(&app).await;
    assert!(view.activity_verified);
    assert!(view.received_message.is_none());
    assert!(started.elapsed() < Duration::from_secs(4));
    app.backend.get_delay_ms.store(0, Ordering::SeqCst);
    assert!(context(&app).await.received_message.is_some());
    assert!(graph.calls.lock().unwrap().is_empty());
}
