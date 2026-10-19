#[path = "support/database.rs"]
mod database;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;
#[path = "fake_provider.rs"]
mod fake_provider;

use std::sync::Arc;

use axum::serve;
use base64::{engine::general_purpose::STANDARD_NO_PAD, Engine as _};
use cmaeg::{
    auth::CreatorAuthenticator,
    channels::management::ManagementState,
    providers::{ProviderClient, ProviderRegistry},
    records::RecordClient,
    router_with_state,
};
use fake_cma::FakeCma;
use fake_nyxid::FakeNyxid;
use fake_provider::{FakeProvider, FakeProviderConfig};
use reqwest::Client;
use serde_json::json;
use tokio::sync::oneshot;

fn registry(provider_url: &str) -> ProviderRegistry {
    ProviderRegistry::parse(&format!(
        r#"[{{"slug":"cma_codex","base_url":"{provider_url}","kind":"responses_http","paths":{{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{{binding_id}}","conversation":"/api/v1/cmaeg/bindings/{{binding_id}}/conversations/{{conversation_id}}","responses":"/api/v2/responses"}},"max_inflight":8}}]"#
    ))
    .unwrap()
}

fn key_ring(current: u8) -> cmaeg_core::seal::MasterKeyRing {
    let old = STANDARD_NO_PAD.encode([1_u8; 32]);
    let new = STANDARD_NO_PAD.encode([2_u8; 32]);
    let spec = if current == 1 {
        format!("1:{old}")
    } else {
        format!("2:{new},1:{old}")
    };
    cmaeg_core::seal::MasterKeyRing::parse(&spec).unwrap()
}
async fn app(
    provider: &FakeProvider,
    cma: &FakeCma,
    nyxid: &FakeNyxid,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    app_with_keys(provider, cma, nyxid, key_ring(1)).await
}
async fn app_with_keys(
    provider: &FakeProvider,
    cma: &FakeCma,
    nyxid: &FakeNyxid,
    ring: cmaeg_core::seal::MasterKeyRing,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    serve_app(management_state(provider, cma, nyxid, ring)).await
}
fn management_state(
    provider: &FakeProvider,
    cma: &FakeCma,
    nyxid: &FakeNyxid,
    ring: cmaeg_core::seal::MasterKeyRing,
) -> ManagementState {
    let providers = Arc::new(
        ProviderClient::new(
            registry(provider.base_url()),
            std::time::Duration::from_secs(1),
            std::time::Duration::from_secs(2),
            8,
        )
        .unwrap(),
    );
    let records = Arc::new(RecordClient::new(&cma.base_url, "cma-service").unwrap());
    let creator_auth = Arc::new(CreatorAuthenticator::new(&nyxid.base_url).unwrap());
    ManagementState::new(
        providers,
        records,
        creator_auth,
        cmaeg::ManagementConfig {
            master_keys: Arc::new(ring),
            public_url: "http://127.0.0.1:8090".into(),
            public_api_url: "http://127.0.0.1:8090".into(),
            cma_base_url: cma.base_url.clone(),
            tools_nyxid_slug: "cmaeg".into(),
            cma_service_token: cmaeg_core::channel::Secret::new("cma-service"),
            nyxid_base_url: nyxid.base_url.clone(),
        },
        cmaeg::ChannelCacheConfig::default(),
    )
}
async fn serve_app(
    state: ManagementState,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        serve(listener, router_with_state(state))
            .with_graceful_shutdown(async move {
                let _ = signal.await;
            })
            .await
            .unwrap();
    });
    (format!("http://{address}"), shutdown, task)
}

fn create_body() -> serde_json::Value {
    json!({
        "schema_version": 1,
        "name": "support-bot",
        "profile": {
            "schema_version": 1,
            "metadata": {
                "name": "test",
                "description": "",
                "owner": {"kind":"human","subject":"alice"},
                "provider": "cma_codex"
            },
            "model": {
                "access":"managed","service":"cma","api_type":"responses",
                "model":"test","thinking_effort":"low"
            },
            "permission": {"action_mode":"guarded","service_scope":[]},
            "context": {"skills":[],"system_prompt":"","mcp_servers":[]},
            "environment": null
        },
        "provider": {"slug":"cma_codex"},
        "sources": [{
            "type":"nyxid_relay","issuer":"https://nyx.example",
            "key_id":"route-key","route_ids":["route-1"],
            "platform":"telegram","admission":{"type":"allowlist","sender_ids":["sender-1"]}
        }],
        "partition": {"version":1,"key":"conversation_and_sender"},
        "reply": {"auto_final":true,"ack_after_ms":3000,"sinks":[{"type":"source"}]},
        "credentials": {
            "agent_key":"nyx_agent_secret",
            "channel_key":"nyx_route_secret",
            "hmac_secrets":[{"key_id":"h1","secret":base64::engine::general_purpose::STANDARD.encode(b"hmac-secret")}]
        }
    })
}

async fn created(client: &Client, base: &str) -> serde_json::Value {
    let response = client
        .post(format!("{base}/v1/channels"))
        .bearer_auth("token-alice")
        .json(&create_body())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 201);
    response.json().await.unwrap()
}

#[path = "management/binary.rs"]
mod binary;
#[path = "management/binary_independent.rs"]
mod binary_independent;
#[path = "management/binary_media.rs"]
mod binary_media;
#[path = "management/binary_routing.rs"]
mod binary_routing;
#[path = "management/binary_support.rs"]
mod binary_support;
#[path = "management/boundaries.rs"]
mod boundaries;
#[path = "management/lifecycle.rs"]
mod lifecycle;
#[path = "management/list.rs"]
mod list;
#[path = "management/operations.rs"]
mod operations;
#[path = "management/platforms.rs"]
mod platforms;
#[path = "management/test_turn.rs"]
mod test_turn;

#[path = "management/invalidation.rs"]
mod invalidation;

#[path = "management/index_hooks.rs"]
mod index_hooks;

#[path = "management/creation.rs"]
mod creation;

#[path = "management/reconciliation.rs"]
mod reconciliation;

#[path = "management/targets.rs"]
mod targets;

#[path = "management/custom_attachments.rs"]
mod custom_attachments;

#[path = "management/audit.rs"]
mod audit;

#[path = "management/write_gate.rs"]
mod write_gate;

#[path = "management/routing.rs"]
mod routing;
