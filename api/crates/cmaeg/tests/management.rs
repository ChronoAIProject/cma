// Fixtures drive the raw driver as independent writers; the gateway's own
// commit and abort go through its guarded wrapper (#3299).
#![allow(clippy::disallowed_methods)]
#[path = "support/database.rs"]
mod database;
#[path = "fake_cma.rs"]
pub mod fake_cma;
#[path = "fake_nyxid.rs"]
pub mod fake_nyxid;
#[path = "fake_provider.rs"]
mod fake_provider;
#[path = "support/management_state.rs"]
mod management_fixture;

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

fn keep_log_callsite_registry_shared() {
    // A callsite first reached by a parallel test with no subscriber must not
    // cache `never` for the only registered thread-scoped capture. Keeping a
    // second dispatcher alive makes tracing consult every live dispatcher.
    static KEEP_REGISTRY_SHARED: std::sync::OnceLock<tracing::Dispatch> =
        std::sync::OnceLock::new();
    KEEP_REGISTRY_SHARED
        .get_or_init(|| tracing::Dispatch::new(tracing::subscriber::NoSubscriber::default()));
}

fn registry(provider_url: &str) -> ProviderRegistry {
    registry_for(provider_url, "cma_codex")
}

fn registry_for(provider_url: &str, slug: &str) -> ProviderRegistry {
    ProviderRegistry::parse(&format!(
        r#"[{{"slug":"{slug}","base_url":"{provider_url}","kind":"responses_http","paths":{{"agent_card":"/.well-known/agent-card.json","binding":"/api/v1/cmaeg/bindings/{{binding_id}}","conversation":"/api/v1/cmaeg/bindings/{{binding_id}}/conversations/{{conversation_id}}","responses":"/api/v2/responses"}},"max_inflight":8}}]"#
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
    management_fixture::state(
        providers,
        records,
        Arc::new(ring),
        &cma.base_url,
        &nyxid.base_url,
    )
}

async fn serve_app(
    state: ManagementState,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    serve_router(router_with_state(state)).await
}
async fn serve_router(
    router: axum::Router,
) -> (String, oneshot::Sender<()>, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let (shutdown, signal) = oneshot::channel();
    let task = tokio::spawn(async move {
        serve(listener, router)
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
#[path = "management/github_integrations.rs"]
mod github_integrations;
#[path = "management/github_write_gate.rs"]
mod github_write_gate;
#[path = "management/health.rs"]
mod health;
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

#[path = "management/recovery.rs"]
mod recovery;

#[path = "management/reconciliation.rs"]
mod reconciliation;

#[path = "management/reconfiguration.rs"]
mod reconfiguration;

#[path = "management/partitions.rs"]
mod partitions;
#[path = "management/targets.rs"]
mod targets;

#[path = "management/custom_attachments.rs"]
mod custom_attachments;

#[path = "management/native_whatsapp.rs"]
mod native_whatsapp;

#[path = "management/audit.rs"]
mod audit;

#[path = "management/write_gate.rs"]
mod write_gate;

#[path = "management/routing.rs"]
mod routing;

#[path = "management/retained_origins.rs"]
mod retained_origins;

#[tokio::test]
async fn channel_endpoints_use_the_advertised_base_in_both_rollout_phases() {
    for (advertise, expected) in [
        (false, "http://127.0.0.1:8090"),
        (true, "https://bot.example/event-gateway"),
    ] {
        let provider = FakeProvider::start(FakeProviderConfig::default()).await;
        let cma = FakeCma::start(false).await;
        let nyxid = FakeNyxid::start().await;
        let mut state = management_state(&provider, &cma, &nyxid, key_ring(1));
        state.public_api_url = "https://bot.example/event-gateway".into();
        state.advertise_public_api_url = advertise;
        let (base, stop, task) = serve_app(state).await;
        let channel = created(&Client::new(), &base).await;
        let id = channel["channel_id"].as_str().unwrap();
        assert_eq!(
            channel["endpoints"]["nyxid_callback_url"],
            format!("{expected}/v1/callbacks/{id}")
        );
        assert_eq!(
            channel["endpoints"]["custom_event_url"],
            format!("{expected}/v1/channels/{id}/events")
        );
        stop.send(()).unwrap();
        task.await.unwrap();
    }
}

#[path = "management/nodes.rs"]
mod nodes;

#[path = "management/outbound.rs"]
mod outbound;

#[path = "management/native_discord.rs"]
mod native_discord;

#[path = "management/native_telegram.rs"]
mod native_telegram;

#[path = "management/native_lark.rs"]
mod native_lark;

#[path = "management/custom_execution.rs"]
mod custom_execution;

#[path = "management/organization_channels.rs"]
mod organization_channels;

#[path = "management/partition_resets.rs"]
mod partition_resets;

#[path = "management/record_management.rs"]
mod record_management;
