#[path = "support/wire.rs"]
mod wire;
use cma_sdk::{Client, Error};
use wire::{Expected, WireServer, headers};

#[tokio::test]
async fn browser_reads_are_bounded_and_bound_to_the_requested_agent_incarnation() {
    let server = WireServer::new().await;
    let body = serde_json::json!({"agent_id":"agent", "runtime_incarnation":"current", "state":"not_enabled",
        "context_id":null,"location":"Blank page","frame":null,"observers":0,"captures":0,"bytes":0,
        "min_interval_ms":1000,"fresh_for_ms":3000,"max_frame_bytes":262144,"max_observers":16});
    let rebuilt = Client::builder(&server.base_url).unwrap().build().unwrap();
    for (client, authenticated) in [
        (&server.anonymous, false),
        (&server.client, true),
        (&rebuilt, false),
    ] {
        for incarnation in ["current", "old"] {
            let mut response = body.clone();
            response["runtime_incarnation"] = serde_json::json!(incarnation);
            server.expect(Expected {
                method: "GET".into(),
                path: "/api/v1/agents/agent/browser?runtime_incarnation=current&observer=reader"
                    .into(),
                headers: headers(authenticated, "application/json"),
                status: 200,
                response: serde_json::to_vec(&response).unwrap(),
                ..Default::default()
            });
        }
        assert_eq!(
            client
                .agent_browser()
                .read("agent", "current", Some("reader"), None)
                .await
                .unwrap()
                .state,
            "not_enabled"
        );
        assert!(matches!(
            client
                .agent_browser()
                .read("agent", "current", Some("reader"), None)
                .await,
            Err(Error::Protocol { .. })
        ));
    }
    server.assert_drained();
    assert_eq!(server.request_count(), 6);
}

#[tokio::test]
async fn browser_address_accepts_only_sanitized_origins_and_retains_old_runtimes() {
    let server = WireServer::new().await;
    for (address, accepted) in [
        (None, true),
        (Some("about:blank"), true),
        (Some("http://localhost:5173"), true),
        (Some("https://example.com"), true),
        (Some("https://example.com/private"), false),
        (Some("https://example.com?private"), false),
        (Some("https://example.com#private"), false),
        (Some("https://user:pass@example.com"), false),
        (Some("file:///home/private"), false),
    ] {
        let mut body = serde_json::json!({"agent_id":"agent", "runtime_incarnation":"current", "state":"not_enabled",
            "context_id":null,"location":"Blank page","frame":null,"observers":0,"captures":0,"bytes":0,
            "min_interval_ms":1000,"fresh_for_ms":3000,"max_frame_bytes":262144,"max_observers":16});
        if let Some(address) = address {
            body["address"] = serde_json::json!(address);
        }
        server.expect(Expected {
            method: "GET".into(),
            path: "/api/v1/agents/agent/browser?runtime_incarnation=current".into(),
            headers: headers(true, "application/json"),
            status: 200,
            response: serde_json::to_vec(&body).unwrap(),
            ..Default::default()
        });
        let result = server
            .client
            .agent_browser()
            .read("agent", "current", None, None)
            .await;
        assert_eq!(result.is_ok(), accepted, "{address:?}");
        if let Ok(observed) = result {
            assert_eq!(observed.address.as_deref(), address);
        }
    }
    server.assert_drained();
}
