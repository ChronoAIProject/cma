use super::tests::{owner, Fixture};
use axum::{extract::State, http::StatusCode};
use serde_json::{json, Value};

#[tokio::test]
async fn relay_authoring_reports_writer_gates_without_expanding_native_connections() {
    let fixture = Fixture::new().await;
    for enabled in [false, true] {
        let mut service = fixture.service();
        service.management.dynamic_sources_writes_enabled = !enabled;
        service.management.relay_raw_verification_writes_enabled = enabled;
        service.management.lark_resource_writes_enabled = enabled;
        service.management.native_discord_thread_history_enabled = enabled;
        service.management.native_discord_thread_lifecycle_enabled = enabled;
        service
            .management
            .native_discord_conversation_history_enabled = enabled;
        service.management.native_lark_conversation_history_enabled = !enabled;
        service.management.native_discord_reactions_enabled = enabled;
        service.management.multi_consumer_writes_enabled = !enabled;
        service.management.native_bot_mentions_writes_enabled = enabled;
        service.management.thread_contract_writes_enabled = !enabled;
        service.management.outbound_messages_writes_enabled = !enabled;
        service.management.outbound_relay = Some(
            crate::delivery::relay_client::RelayClient::new(
                "https://relay.example",
                std::time::Duration::from_secs(1),
            )
            .unwrap()
            .with_native_media(enabled),
        );
        let response =
            crate::http::connections::capabilities(State(service), owner("alice", false)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        assert_eq!(value["platforms"].as_array().unwrap().len(), 1);
        assert_eq!(value["platforms"][0]["id"], "telegram");
        let relay = &value["relay_authoring"];
        assert_eq!(relay["dynamic_sources_writes_enabled"], !enabled);
        assert_eq!(relay["relay_raw_verification_writes_enabled"], enabled);
        assert_eq!(relay["lark_resource_writes_enabled"], enabled);
        assert_eq!(relay["native_discord_thread_history_enabled"], enabled);
        assert_eq!(relay["native_discord_thread_lifecycle_enabled"], enabled);
        assert_eq!(
            relay["native_discord_conversation_history_enabled"],
            enabled
        );
        assert_eq!(relay["native_lark_conversation_history_enabled"], !enabled);
        assert_eq!(relay["native_discord_reactions_enabled"], enabled);
        assert_eq!(relay["multi_consumer_writes_enabled"], !enabled);
        assert_eq!(relay["native_bot_mentions_writes_enabled"], enabled);
        assert_eq!(relay["thread_contract_writes_enabled"], !enabled);
        assert_eq!(relay["outbound_messages_writes_enabled"], !enabled);
        assert_eq!(relay["native_media_writes_enabled"], enabled);
        let platforms = relay["platforms"].as_array().unwrap();
        assert_eq!(platforms.len(), 6);
        for platform in platforms {
            let expected = match platform["id"].as_str().unwrap() {
                "discord" | "whatsapp" => json!([
                    {"id":"raw_event", "writes_enabled":enabled},
                    {"id":"trusted_normalized", "writes_enabled":true}
                ]),
                "slack" => json!([{"id":"trusted_normalized", "writes_enabled":true}]),
                _ => json!([{"id":"native_event", "writes_enabled":true}]),
            };
            assert_eq!(platform["verification_modes"], expected);
        }
    }
}

#[tokio::test]
async fn native_whatsapp_capabilities_separate_authoring_and_durable_activation_prerequisites() {
    let fixture = Fixture::new().await;
    for bits in 0..8 {
        let mut service = fixture.service();
        service.management.native_whatsapp_writes_enabled = bits & 1 != 0;
        service.management.native_whatsapp_admission_enabled = bits & 2 != 0;
        service.management.multi_consumer_writes_enabled = bits & 4 != 0;
        let response =
            crate::http::connections::capabilities(State(service), owner("alice", false)).await;
        assert_eq!(response.status(), StatusCode::OK);
        assert_eq!(response.headers()["cache-control"], "no-store");
        let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
            .await
            .unwrap();
        let value: Value = serde_json::from_slice(&bytes).unwrap();
        let authoring = &value["relay_authoring"];
        assert_eq!(authoring["native_whatsapp_writes_enabled"], bits & 1 != 0);
        assert_eq!(
            authoring["native_whatsapp_auto_activation_enabled"],
            bits & 6 == 6
        );
        assert_eq!(value["platforms"][0]["id"], "telegram");
    }
}

#[tokio::test]
async fn missing_relay_does_not_advertise_native_media() {
    let fixture = Fixture::new().await;
    let mut service = fixture.service();
    service.management.outbound_messages_writes_enabled = true;
    let response =
        crate::http::connections::capabilities(State(service), owner("alice", false)).await;
    let bytes = axum::body::to_bytes(response.into_body(), 64 * 1024)
        .await
        .unwrap();
    let value: Value = serde_json::from_slice(&bytes).unwrap();
    assert_eq!(
        value["relay_authoring"]["native_media_writes_enabled"],
        false
    );
}

#[tokio::test]
async fn capability_and_owned_connection_discovery_require_authentication() {
    let fixture = Fixture::new().await;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let service = fixture.service();
    let server = tokio::spawn(async move {
        axum::serve(listener, crate::http::connections::router(service))
            .await
            .unwrap();
    });
    for path in ["/v1/channel-capabilities", "/v1/channel-connections"] {
        let response = reqwest::get(format!("http://{address}{path}"))
            .await
            .unwrap();
        assert_eq!(response.status(), StatusCode::UNAUTHORIZED);
    }
    server.abort();
}
