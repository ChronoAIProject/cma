//! Exercise the actual runner with both upstream routes and a Father binding.
use super::{tests::activity, BotFatherClient};
use crate::{
    channels::resolver::ResolvedChannel,
    http::readiness::Readiness,
    ingress::launch::{LaunchOutcome, LaunchRequest, TurnLauncher},
    providers::{ProviderClient, ProviderRegistry},
    turns::{outcomes::DeliveryCommand, TurnRunner, TurnTracker},
    TurnConfig,
};
use axum::{extract::State, http::StatusCode, routing::post, Json, Router};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg_core::{
    channel::{ChannelDefinition, Secret},
    ids::{ConversationId, IdempotencyKey},
    seal::{ChannelPointer, EventRef, EventRefPayload, MasterKeyRing, ReplyAddress},
};
use serde_json::{json, Value};
use std::{
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

#[derive(Clone, Default)]
struct Calls {
    provider: Arc<Mutex<Vec<Value>>>,
    setup: Arc<Mutex<Vec<Value>>>,
}
async fn provider(State(calls): State<Calls>, Json(body): Json<Value>) -> StatusCode {
    calls.provider.lock().unwrap().push(body);
    StatusCode::SERVICE_UNAVAILABLE
}
async fn setup(State(calls): State<Calls>, Json(body): Json<Value>) -> Json<Value> {
    calls.setup.lock().unwrap().push(body);
    Json(
        json!({"step":"login", "configuration":{}, "choices":[], "connection_url":"https://identity.example/login"}),
    )
}

#[tokio::test]
async fn bot_father_runner_dispatches_only_explicit_controls_and_keeps_chat_available() {
    let calls = Calls::default();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let app = Router::new()
        .route("/api/v2/responses", post(provider))
        .route("/api/v1/bot-setups/advance", post(setup))
        .with_state(calls.clone());
    let server = tokio::spawn(async move { axum::serve(listener, app).await.unwrap() });
    let registry = ProviderRegistry::parse(&json!([{
        "slug":"cma_codex", "base_url":base, "kind":"responses_http",
        "paths":{"agent_card":"/.well-known/agent-card.json", "binding":"/api/v1/cmaeg/bindings/{binding_id}", "conversation":"/api/v1/cmaeg/bindings/{binding_id}/conversations/{conversation_id}", "responses":"/api/v2/responses"},
        "max_inflight":1
    }]).to_string()).unwrap();
    let providers = Arc::new(
        ProviderClient::new(registry, Duration::from_secs(1), Duration::from_secs(1), 1).unwrap(),
    );
    let (delivery, mut outcomes) = tokio::sync::mpsc::channel(8);
    let keys = MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap();
    let mut definition: ChannelDefinition = serde_json::from_str(include_str!(
        "../../../../cmaeg-core/tests/fixtures/channel.json"
    ))
    .unwrap();
    definition.profile_digest = definition.profile.digest();
    definition.secrets.provider_credential = Some(Secret::new("fixture-provider-credential"));
    let runner = TurnRunner::new(
        providers,
        TurnTracker::new(Readiness::initialized()),
        delivery,
        TurnConfig::default(),
    )
    .with_bot_father(Some(BotFatherClient {
        http: reqwest::Client::new(),
        endpoint: format!("{base}/api/v1/bot-setups/advance").parse().unwrap(),
        binding_id: definition.provider.binding_id.to_string(),
    }));
    let pointer = ChannelPointer {
        v: 1,
        record_id: definition.record_id.clone(),
        record_url: "https://cma.example/api/v1/gateway/channel-records/fixture".into(),
    };
    let channel = Arc::new(ResolvedChannel {
        id: pointer.seal_id(&keys).unwrap(),
        pointer,
        definition,
        agent_key_hash: [0; 32],
        loaded_at: Instant::now(),
        version: 1,
    });
    let cases = [
        ("hi", false),
        ("How's the weather today?", false),
        ("Help me create a Telegram bot", false),
        ("{\"agent\":{}}", false),
        ("```json\n{\"agent\":{}}\n```", false),
        ("done", false),
        ("create", false),
        ("/start", false),
        ("/setup name: Paper assistant", true),
        ("hi again", false),
        ("/setup create", true),
    ];
    for (index, (text, uses_setup)) in cases.iter().enumerate() {
        let mut activity = activity(text);
        activity.event_id = format!("event-{index}").parse().unwrap();
        let partition = activity.partition_key(&channel.definition.partition);
        let conversation = ConversationId::derive(&keys, &channel.id, 1, &partition);
        let event_ref = EventRef::seal(
            &EventRefPayload {
                v: 1,
                activity_sha256: Some(cmaeg_core::seal::event_ref::activity_digest(&activity)),
                channel_id: channel.id.clone(),
                record_id: channel.pointer.record_id.clone(),
                event_id: activity.event_id.clone(),
                partition_key: partition,
                conversation_id: conversation.clone(),
                expires_at: time::OffsetDateTime::now_utc().unix_timestamp() + 1800,
                reply: ReplyAddress::None,
            },
            &keys,
        )
        .unwrap();
        let request = LaunchRequest {
            idempotency_key: IdempotencyKey::from_event(&activity.event_id),
            channel: channel.clone(),
            activity,
            conversation_id: conversation,
            event_ref,
            received_at: Instant::now(),
            trace: tracing::Span::current(),
            timing: None,
        };
        let result = runner.launch(request).await;
        assert_eq!(
            matches!(result, LaunchOutcome::Accepted { .. }),
            *uses_setup,
            "{text}"
        );
        let DeliveryCommand::StreamOutcome { outcome, .. } =
            tokio::time::timeout(Duration::from_secs(2), outcomes.recv())
                .await
                .unwrap()
                .unwrap()
        else {
            panic!("expected outcome")
        };
        assert!(outcome.terminal());
    }
    assert_eq!(
        calls.provider.lock().unwrap().len(),
        cases.iter().filter(|(_, setup)| !setup).count()
    );
    let setup_calls = calls.setup.lock().unwrap();
    assert_eq!(setup_calls.len(), 2);
    assert_eq!(setup_calls[0]["draft"]["name"], "Paper assistant");
    assert_eq!(
        setup_calls[0]["activity"]["kind"]["text"],
        "/setup name: Paper assistant"
    );
    assert_eq!(setup_calls[1]["activity"]["kind"]["text"], "/setup create");
    server.abort();
}
