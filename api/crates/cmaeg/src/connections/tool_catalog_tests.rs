use super::tests::{owner, Fixture};
use crate::{
    auth::CreatorAuthenticator,
    media::{
        budget::InflightBytes,
        service::{MediaDeps, MediaService},
        source::{inline::InlineSource, SourceRegistry},
        transcribe::{meter::SttSpend, upload::NoTranscoder, DisabledTranscriber},
        visual::PlatformThumbnail,
        MediaLimits,
    },
    tools::{DiscordEffectsClient, LarkEffectsClient, ToolsState},
    MediaConfig, SttConfig,
};
use axum::{
    extract::State,
    http::{HeaderMap, StatusCode},
    routing::get,
    Json, Router,
};
use serde_json::{json, Value};
use std::sync::Arc;

fn media(state: &ToolsState) -> Arc<MediaService> {
    let inline = Arc::new(InlineSource);
    let sources = Arc::new(SourceRegistry::new(
        inline.clone(),
        inline.clone(),
        inline.clone(),
        inline,
    ));
    let decode = Arc::new(tokio::sync::Semaphore::new(1));
    let stt = SttConfig::default();
    let transcriber = Arc::new(DisabledTranscriber);
    let limits = MediaLimits::from_config(&MediaConfig::default());
    Arc::new(MediaService::new(MediaDeps {
        sources: sources.clone(),
        transcoder: Arc::new(NoTranscoder::new(&stt, transcriber.clone())),
        transcriber,
        stt: Arc::new(SttSpend::new(&stt)),
        visuals: Arc::new(PlatformThumbnail::new(sources, decode.clone())),
        limits,
        keys: state.keys.clone(),
        tool_providers: Arc::default(),
        decode,
        inflight: InflightBytes::new(&limits),
    }))
}
async fn serve(router: Router) -> (String, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let task = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    (base, task)
}
fn projection(tool: &Value) -> Value {
    json!({"name":tool["name"], "description":tool["description"],
        "read_only":tool["annotations"]["readOnlyHint"],
        "destructive":tool["annotations"]["destructiveHint"],
        "idempotent":tool["annotations"]["idempotentHint"],
        "open_world":tool["annotations"]["openWorldHint"]})
}

#[tokio::test]
async fn owner_tool_catalog_equals_mcp_metadata_under_every_discovery_gate() {
    let fixture = Fixture::new().await;
    for enabled_media in [false, true] {
        for gate in [
            "none",
            "telegram",
            "discord",
            "discord_card",
            "lark",
            "lark_client_only",
        ] {
            let mut service = fixture.service();
            service.tools.telegram_effects_issuance_enabled = gate == "telegram";
            service.tools.discord_effects_issuance_enabled = gate == "discord";
            service.tools.discord_effects = Some(Arc::new(
                DiscordEffectsClient::new(crate::discord::DiscordRest::new().unwrap())
                    .with_card_effects(gate == "discord_card"),
            ));
            service.tools.lark_effects = Some(Arc::new(
                LarkEffectsClient::new("https://nyx.example").unwrap(),
            ));
            service.tools.lark_effects_issuance_enabled = gate == "lark";
            if enabled_media {
                service.tools.media = Some(media(&service.tools));
            }
            let (base, server) = serve(crate::mcp::router(service.tools.clone())).await;
            let http = reqwest::Client::new();
            let mcp: Value = http
                .post(format!("{base}/mcp"))
                .bearer_auth("nyxid_ag_catalog-agent-fixture")
                .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
                .send()
                .await
                .unwrap()
                .json()
                .await
                .unwrap();
            let response =
                crate::http::connections::capabilities(State(service), owner("alice", false)).await;
            assert_eq!(response.headers()["cache-control"], "no-store");
            let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
                .await
                .unwrap();
            let catalog: Value = serde_json::from_slice(&bytes).unwrap();
            let expected: Vec<_> = mcp["result"]["tools"]
                .as_array()
                .unwrap()
                .iter()
                .map(projection)
                .collect();
            assert_eq!(
                catalog["agent_tools"],
                json!(expected),
                "{gate}/{enabled_media}"
            );
            let expected_effects = !matches!(gate, "none" | "lark_client_only");
            assert_eq!(
                expected.len(),
                4 + usize::from(enabled_media) + if expected_effects { 3 } else { 0 }
            );
            assert_eq!(
                expected
                    .iter()
                    .any(|tool| tool["name"] == "readEventAttachment"),
                enabled_media
            );
            assert_eq!(
                expected.iter().any(|tool| tool["name"] == "deleteMessage"),
                expected_effects
            );
            // Listing a server tool grants no authority over an arbitrary message.
            let refused: Value = http.post(format!("{base}/mcp"))
                .bearer_auth("nyxid_ag_catalog-agent-fixture")
                .json(&json!({"jsonrpc":"2.0","id":2,"method":"tools/call","params":{"name":"deleteMessage","arguments":{"message_ref":"forged","idempotency_key":"catalog"}}}))
                .send().await.unwrap().json().await.unwrap();
            assert!(refused.get("error").is_some());
            server.abort();
        }
    }
}

#[tokio::test]
async fn tool_catalog_requires_owner_authentication_without_agent_custody() {
    let auth = Router::new().route(
        "/api/v1/users/me",
        get(|headers: HeaderMap| async move {
            if headers
                .get("authorization")
                .and_then(|value| value.to_str().ok())
                == Some("Bearer owner-fixture")
            {
                (
                    StatusCode::OK,
                    Json(json!({"id":"alice","user_type":"person"})),
                )
            } else {
                (StatusCode::UNAUTHORIZED, Json(json!({})))
            }
        }),
    );
    let (auth_base, auth_server) = serve(auth).await;
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.creator_auth = Arc::new(CreatorAuthenticator::new(&auth_base).unwrap());
    let (base, server) = serve(crate::http::connections::router(service)).await;
    let http = reqwest::Client::new();
    for credential in [
        None,
        Some("nyxid_ag_catalog-agent-fixture"),
        Some("invalid-fixture"),
    ] {
        let mut request = http.get(format!("{base}/v1/channel-capabilities"));
        if let Some(value) = credential {
            request = request.bearer_auth(value);
        }
        let response = request.send().await.unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
        assert!(response
            .json::<Value>()
            .await
            .unwrap()
            .get("agent_tools")
            .is_none());
    }
    let response = http
        .get(format!("{base}/v1/channel-capabilities"))
        .bearer_auth("owner-fixture")
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert_eq!(response.headers()["cache-control"], "no-store");
    assert_eq!(
        response.json::<Value>().await.unwrap()["agent_tools"]
            .as_array()
            .unwrap()
            .len(),
        4
    );
    server.abort();
    auth_server.abort();
}
