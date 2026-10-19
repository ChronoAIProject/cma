use super::support::{problem, App};
use serde_json::{json, Value};
#[tokio::test]
async fn mcp_initializes_lists_generated_schemas_and_uses_the_same_receipts() {
    let app = App::new().await;
    let response=app.rpc(json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{"protocolVersion":"2025-06-18","capabilities":{},"clientInfo":{"name":"test","version":"1"}}})).await;
    assert_eq!(response.headers()["mcp-protocol-version"], "2025-06-18");
    let result = response.json::<Value>().await.unwrap();
    assert_eq!(result["result"]["serverInfo"]["name"], "cmaeg");
    let list = app
        .rpc(json!({"jsonrpc":"2.0","id":2,"method":"tools/list"}))
        .await
        .json::<Value>()
        .await
        .unwrap();
    let tools = list["result"]["tools"].as_array().unwrap();
    assert_eq!(tools.len(), 4);
    for tool in tools {
        assert!(tool["inputSchema"]["properties"]["event_ref"].is_object());
        assert!(tool["inputSchema"]["required"]
            .as_array()
            .unwrap()
            .contains(&json!("event_ref")));
        assert!(!tool["inputSchema"].to_string().contains("#/components/"));
    }
    let reply=app.rpc(json!({"jsonrpc":"2.0","id":3,"method":"tools/call","params":{"name":"replyToEvent","arguments":{"event_ref":app.event_ref.as_str(),"message":{"text":"through MCP"}}}})).await.json::<Value>().await.unwrap();
    assert_eq!(
        reply["result"]["structuredContent"]["delivery"]["outcome"],
        "confirmed"
    );
    assert_eq!(reply["result"]["isError"], false);
    assert_eq!(
        app.requests.lock().unwrap()[0].text.as_deref(),
        Some("through MCP")
    );
}
#[tokio::test]
async fn mcp_errors_auth_headers_notifications_and_methods_do_not_send() {
    let app = App::new().await;
    let bad=app.rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"replyToEvent","arguments":{"event_ref":"bad","message":{"text":"no"}}}})).await.json::<Value>().await.unwrap();
    assert_eq!(bad["error"]["data"]["code"], "event_ref_invalid");
    let unknown = app
        .rpc(json!({"jsonrpc":"2.0","id":"a","method":"missing"}))
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_eq!(unknown["error"]["code"], -32601);
    let notification=app.rpc(json!({"jsonrpc":"2.0","method":"tools/call","params":{"name":"replyToEvent","arguments":{"event_ref":app.event_ref.as_str(),"message":{"text":"no notification send"}}}})).await;
    assert_eq!(notification.status(), 202);
    assert!(app.requests.lock().unwrap().is_empty());
    let url = format!("{}/mcp", app.base);
    problem(
        app.fixture
            .client
            .post(&url)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
            .send()
            .await
            .unwrap(),
        401,
        "not_authenticated",
    )
    .await;
    for method in [reqwest::Method::GET, reqwest::Method::DELETE] {
        problem(
            app.fixture
                .client
                .request(method, &url)
                .bearer_auth(&app.key)
                .send()
                .await
                .unwrap(),
            405,
            "invalid_request",
        )
        .await;
    }
    problem(
        app.fixture
            .client
            .post(&url)
            .bearer_auth(&app.key)
            .header("origin", "https://untrusted.example")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
            .send()
            .await
            .unwrap(),
        403,
        "invalid_request",
    )
    .await;
    problem(
        app.fixture
            .client
            .post(&url)
            .bearer_auth(&app.key)
            .header("mcp-protocol-version", "unknown")
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
            .send()
            .await
            .unwrap(),
        400,
        "invalid_request",
    )
    .await;
}

#[tokio::test]
async fn mcp_update_requires_both_references_to_name_the_same_event() {
    let app = App::new().await;
    let mut payload = app.message_ref.open(&app.state.keys).unwrap().payload;
    payload.event_id = "different-event".parse().unwrap();
    let foreign = cmaeg_core::ids::MessageRef::seal(&payload, &app.state.keys).unwrap();
    let response=app.rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/call","params":{"name":"updateMessage","arguments":{"event_ref":app.event_ref.as_str(),"message_ref":foreign,"message":{"text":"change"}}}})).await.json::<Value>().await.unwrap();
    assert_eq!(response["error"]["data"]["code"], "forbidden");
    for (method, code) in [
        (json!({"jsonrpc":"1.0","id":1,"method":"ping"}), -32600),
        (json!({"jsonrpc":"2.0","id":{},"method":"ping"}), -32600),
        (
            json!({"jsonrpc":"2.0","id":1,"method":"initialize","params":{}}),
            -32602,
        ),
    ] {
        let result = app.rpc(method).await.json::<Value>().await.unwrap();
        assert_eq!(result["error"]["code"], code);
    }
}

/// `tools/list` captured before tool metadata moved into a per-tool table; any
/// difference in names, descriptions, schemas or hints is a wire change.
const TOOLS_LIST_GOLDEN: &str = include_str!("../fixtures/mcp-tools-list.json");

#[tokio::test]
async fn tools_list_matches_the_pre_refactor_golden_document() {
    let app = App::new().await;
    let list = app
        .rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .json::<Value>()
        .await
        .unwrap();
    let golden: Value = serde_json::from_str(TOOLS_LIST_GOLDEN).unwrap();
    assert_eq!(list["result"], golden);
}

#[tokio::test]
async fn mcp_migration_accepts_exact_canonical_and_legacy_origins() {
    let app = App::new().await;
    let mut state = app.state.clone();
    state.public_api_url = "https://bot.example/event-gateway".into();
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let url = format!("http://{}/mcp", listener.local_addr().unwrap());
    let server = tokio::spawn(async move {
        axum::serve(listener, cmaeg::mcp::router(state))
            .await
            .unwrap()
    });
    for (origin, status) in [
        ("https://bot.example", 200),
        ("https://gateway.example", 200),
        ("https://bot.example/event-gateway", 403),
        ("https://bot.example.evil.test", 403),
        ("null", 403),
    ] {
        let response = app
            .fixture
            .client
            .post(&url)
            .bearer_auth(&app.key)
            .header("origin", origin)
            .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
            .send()
            .await
            .unwrap();
        assert_eq!(response.status().as_u16(), status, "{origin}");
    }
    let response = app
        .fixture
        .client
        .post(&url)
        .bearer_auth(&app.key)
        .header("origin", "https://bot.example")
        .header("origin", "https://gateway.example")
        .json(&json!({"jsonrpc":"2.0","id":1,"method":"ping"}))
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), 403);
    server.abort();
}
