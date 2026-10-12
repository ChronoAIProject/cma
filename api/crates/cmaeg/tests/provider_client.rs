#[path = "fake_provider.rs"]
mod fake_provider;

use std::{collections::BTreeMap, time::Duration};

use cmaeg::providers::{
    BindingRequest, ProviderClient, ProviderError, ProviderRegistry, ResponsesEvent,
    ToolsAdvertisement, TurnRequest,
};
use cmaeg_core::{
    channel::Secret,
    ids::{BindingId, ConversationId, IdempotencyKey, ProviderSlug},
    profile::{
        ActionMode, ApiType, ContextConfig, EventWorkerProfile, ModelAccess::Managed, ModelConfig,
        NyxIdIdentity, NyxIdIdentityKind, PermissionConfig, ProfileMetadata, ThinkingEffort,
    },
};
use fake_provider::{FakeProvider, FakeProviderConfig};

fn registry(base_url: &str, max_inflight: usize) -> ProviderRegistry {
    ProviderRegistry::parse(&format!(
        r#"[{{"slug":"cma_codex","base_url":"{base_url}","kind":"responses_http","paths":{{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{{binding_id}}","conversation":"/api/v1/cmaeg/bindings/{{binding_id}}/conversations/{{conversation_id}}","responses":"/api/v2/responses"}},"max_inflight":{max_inflight}}}]"#
    ))
    .unwrap()
}

fn provider() -> ProviderSlug {
    ProviderSlug::new("cma_codex").unwrap()
}

fn profile() -> EventWorkerProfile {
    EventWorkerProfile {
        schema_version: 1,
        metadata: ProfileMetadata {
            name: "test".into(),
            description: String::new(),
            owner: NyxIdIdentity {
                kind: NyxIdIdentityKind::Human,
                subject: "owner".into(),
            },
            provider: provider(),
        },
        model: ModelConfig {
            access: Managed,
            service: "cma".into(),
            api_type: ApiType::Responses,
            model: "test".into(),
            thinking_effort: ThinkingEffort::Low,
        },
        permission: PermissionConfig {
            action_mode: ActionMode::Guarded,
            service_scope: Vec::new(),
        },
        context: ContextConfig {
            skills: Vec::new(),
            system_prompt: String::new(),
            mcp_servers: Vec::new(),
        },
        environment: None,
    }
}

fn client(fake: &FakeProvider, max_inflight: usize) -> ProviderClient {
    ProviderClient::new(
        registry(fake.base_url(), max_inflight),
        Duration::from_secs(1),
        Duration::from_secs(2),
        max_inflight,
    )
    .unwrap()
}

#[tokio::test]
async fn bind_conversation_probe_and_stream_round_trip() {
    let fake = FakeProvider::start(FakeProviderConfig::default()).await;
    let client = client(&fake, 4);
    let provider = provider();
    let binding = BindingId::new("bind-1").unwrap();
    let conversation = ConversationId::new("conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let secret = Secret::new("agent-secret");
    let binding_response = client
        .bind(
            &provider,
            &binding,
            &BindingRequest {
                schema_version: 1,
                target: Default::default(),
                profile: profile(),
                profile_digest: "digest".into(),
                agent_key: secret.clone(),
                tools: ToolsAdvertisement {
                    native_mcp_url: "https://tools.example".into(),
                    nyxid_service_slug: "cmaeg".into(),
                },
            },
        )
        .await
        .unwrap();
    assert_eq!(binding_response.binding_id, binding);
    assert_eq!(
        client.probe(&provider).await.unwrap().level,
        cmaeg_core::channel::ConformanceLevel::L5
    );
    client
        .ensure_conversation(&provider, &binding, &conversation, &secret)
        .await
        .unwrap();
    let mut stream = client
        .start_turn(
            &provider,
            TurnRequest {
                timing: None,
                conversation_id: conversation.clone(),
                input: vec![serde_json::json!({"role":"user","content":"hello"})],
                idempotency_key: IdempotencyKey::from_event(&"event-1".parse().unwrap()),
                metadata: BTreeMap::new(),
                agent_key: secret.clone(),
                event_context: None,
            },
        )
        .await
        .unwrap();
    let request_started = stream.request_started;
    let accepted = stream.accepted.await.unwrap();
    assert_eq!(accepted.response_id, "resp_fake");
    assert!(accepted.at.duration_since(request_started) < Duration::from_millis(50));
    let mut events = Vec::new();
    while let Some(event) = stream.events.recv().await {
        events.push(event);
    }
    assert!(matches!(
        events.first(),
        Some(ResponsesEvent::Created { .. })
    ));
    assert!(matches!(
        events.last(),
        Some(ResponsesEvent::Completed { .. })
    ));
    client
        .delete_conversation(&provider, &binding, &conversation, &secret)
        .await
        .unwrap();
    client.unbind(&provider, &binding, &secret).await.unwrap();
    let requests = fake.requests();
    let bind = requests
        .iter()
        .find(|request| request.method == "PUT" && !request.path.contains("conversations"))
        .unwrap();
    let forwarded: serde_json::Value = serde_json::from_str(&bind.body).unwrap();
    assert!(
        forwarded.get("target").is_none(),
        "legacy providers must receive the original request shape"
    );
    assert!(requests
        .iter()
        .any(|request| request.authorization.as_deref() == Some("Bearer agent-secret")));
    assert!(requests
        .iter()
        .any(|request| request.idempotency_key.as_deref() == Some("evt_event-1")));
    assert!(requests
        .iter()
        .filter(|request| request.method != "PUT" || request.path.contains("conversations"))
        .all(|request| !request.body.contains("agent-secret")));
    fake.close().await;
}

#[tokio::test]
async fn busy_and_missing_conversation_errors_are_typed() {
    let fake = FakeProvider::start(FakeProviderConfig {
        busy_once: true,
        conversation_missing_once: true,
        ..FakeProviderConfig::default()
    })
    .await;
    let client = client(&fake, 2);
    let provider = provider();
    let binding = BindingId::new("bind-1").unwrap();
    let conversation = ConversationId::new("conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let secret = Secret::new("agent-secret");
    assert!(matches!(
        client
            .ensure_conversation(&provider, &binding, &conversation, &secret)
            .await,
        Err(ProviderError::ConversationMissing)
    ));
    let error = client
        .start_turn(
            &provider,
            TurnRequest {
                timing: None,
                conversation_id: conversation,
                input: Vec::new(),
                idempotency_key: IdempotencyKey::from_event(&"event-2".parse().unwrap()),
                metadata: BTreeMap::new(),
                agent_key: secret,
                event_context: None,
            },
        )
        .await
        .unwrap_err();
    assert!(matches!(error, ProviderError::Busy { .. }));
    fake.close().await;
}

#[tokio::test]
async fn full_provider_semaphore_returns_busy_without_waiting() {
    let fake = FakeProvider::start(FakeProviderConfig {
        response_delay: Duration::from_millis(200),
        ..FakeProviderConfig::default()
    })
    .await;
    let client = client(&fake, 1);
    let provider = provider();
    let secret = Secret::new("agent-secret");
    let request = |event: &str| TurnRequest {
        timing: None,
        conversation_id: ConversationId::new("conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        input: Vec::new(),
        idempotency_key: IdempotencyKey::from_event(&event.parse().unwrap()),
        metadata: BTreeMap::new(),
        agent_key: secret.clone(),
        event_context: None,
    };
    let first = client
        .start_turn(&provider, request("event-1"))
        .await
        .unwrap();
    let started = std::time::Instant::now();
    let second = client.start_turn(&provider, request("event-2")).await;
    assert!(matches!(second, Err(ProviderError::Busy { .. })));
    assert!(started.elapsed() < Duration::from_millis(150));
    drop(first);
    fake.close().await;
}

#[tokio::test]
async fn one_hundred_sequential_turns_accept_with_low_latency() {
    let fake = FakeProvider::start(FakeProviderConfig::default()).await;
    let client = client(&fake, 4);
    let provider = provider();
    let secret = Secret::new("agent-secret");
    let conversation_id = ConversationId::new("conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap();
    let mut latencies = Vec::with_capacity(100);
    for index in 0..100 {
        let mut stream = client
            .start_turn(
                &provider,
                TurnRequest {
                    timing: None,
                    conversation_id: conversation_id.clone(),
                    input: Vec::new(),
                    idempotency_key: IdempotencyKey::from_event(
                        &format!("event-{index}").parse().unwrap(),
                    ),
                    metadata: BTreeMap::new(),
                    agent_key: secret.clone(),
                    event_context: None,
                },
            )
            .await
            .unwrap();
        let accepted = stream.accepted.await.unwrap();
        latencies.push(accepted.at.duration_since(stream.request_started));
        while stream.events.recv().await.is_some() {}
    }
    latencies.sort_unstable();
    eprintln!(
        "100 sequential provider acceptance median: {:?}",
        latencies[49]
    );
    assert!(latencies[49] < Duration::from_millis(5));
    fake.close().await;
}

#[tokio::test]
async fn streaming_lifetime_outlives_header_timeout_and_drop_releases_capacity() {
    let fake = FakeProvider::start(FakeProviderConfig {
        response_delay: Duration::from_millis(250),
        ..Default::default()
    })
    .await;
    let client = ProviderClient::new(
        registry(fake.base_url(), 1),
        Duration::from_secs(1),
        Duration::from_millis(100),
        1,
    )
    .unwrap();
    let request = |event: &str| TurnRequest {
        timing: None,
        conversation_id: ConversationId::new("conv_aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa").unwrap(),
        input: vec![],
        idempotency_key: IdempotencyKey::from_event(&event.parse().unwrap()),
        metadata: BTreeMap::new(),
        agent_key: Secret::new("ephemeral-test-agent"),
        event_context: None,
    };
    let mut first = client
        .start_turn_immediate(&provider(), request("one"))
        .await
        .unwrap();
    assert!(matches!(
        client
            .start_turn_immediate(&provider(), request("two"))
            .await,
        Err(ProviderError::Capacity)
    ));
    assert_eq!(
        (&mut first.accepted).await.unwrap().response_id,
        "resp_fake"
    );
    while first.events.recv().await.is_some() {}
    let idle = client
        .start_turn_immediate(&provider(), request("three"))
        .await
        .unwrap();
    drop(idle);
    let start = std::time::Instant::now();
    let next = tokio::time::timeout(Duration::from_millis(100), async {
        loop {
            match client
                .start_turn_immediate(&provider(), request("four"))
                .await
            {
                Err(ProviderError::Capacity) => tokio::task::yield_now().await,
                result => break result.unwrap(),
            }
        }
    })
    .await
    .unwrap();
    assert!(start.elapsed() < Duration::from_millis(100));
    drop(next);
    assert_eq!(fake.requests().len(), 3);
    fake.close().await;
}
