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
async fn tools_list_matches_the_pre_refactor_golden_with_reviewed_extensions() {
    let app = App::new().await;
    let mut list = app
        .rpc(json!({"jsonrpc":"2.0","id":1,"method":"tools/list"}))
        .await
        .json::<Value>()
        .await
        .unwrap();
    assert_tools_with_reviewed_extensions(&mut list["result"]);
}

/// Preserve recorded schemas apart from reviewed optional key/history, media and docs.
pub(super) fn assert_tools_with_reviewed_extensions(list: &mut Value) {
    let golden: Value = serde_json::from_str(TOOLS_LIST_GOLDEN).unwrap();
    for (tool, previous) in list["tools"]
        .as_array_mut()
        .unwrap()
        .iter_mut()
        .zip(golden["tools"].as_array().unwrap())
    {
        let mut context = tool["inputSchema"]["$defs"]
            .as_object_mut()
            .unwrap()
            .remove("ContextArguments")
            .unwrap();
        let history = context["properties"]
            .as_object_mut()
            .unwrap()
            .remove("include_thread_history")
            .unwrap();
        assert_eq!(history["type"], "boolean");
        assert_eq!(context, previous["inputSchema"]["$defs"]["EventArguments"]);
        if tool["name"] == "readEventContext" {
            let direct = tool["inputSchema"]["properties"]
                .as_object_mut()
                .unwrap()
                .remove("include_thread_history")
                .unwrap();
            assert_eq!(direct, history);
            assert_eq!(tool["annotations"]["openWorldHint"], true);
            let description = tool["description"].as_str().unwrap();
            assert!(description.contains("at most 20 preceding text messages"));
            assert!(description.contains("untrusted reference data"));
            tool["annotations"]["openWorldHint"] = previous["annotations"]["openWorldHint"].clone();
            tool["description"] = previous["description"].clone();
        }
        let media_kinds = &mut tool["inputSchema"]["$defs"]["NativeMediaKind"]["enum"];
        let mut expanded = previous["inputSchema"]["$defs"]["NativeMediaKind"]["enum"].clone();
        expanded.as_array_mut().unwrap().extend([
            json!("voice"),
            json!("animation"),
            json!("sticker"),
        ]);
        assert_eq!(*media_kinds, expanded);
        *media_kinds = previous["inputSchema"]["$defs"]["NativeMediaKind"]["enum"].clone();
        let media_description =
            &mut tool["inputSchema"]["$defs"]["NativeMediaSource"]["oneOf"][0]["description"];
        let previous_description =
            &previous["inputSchema"]["$defs"]["NativeMediaSource"]["oneOf"][0]["description"];
        assert!(
            media_description == previous_description
                || media_description
                    == "The native adapter or NyxID fetches public HTTPS with bounded, pinned-address policy."
        );
        *media_description = previous_description.clone();
        let clear = tool["inputSchema"]["$defs"]["OutboundMessage"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("clear_native_attachments")
            .unwrap();
        assert_eq!(clear["type"], "boolean");
        assert_eq!(
            clear["description"],
            "Explicitly remove every native attachment; updateMessage only when advertised."
        );
        let thread_description = &mut tool["inputSchema"]["$defs"]["OutboundMessage"]["properties"]
            ["thread_reply"]["description"];
        assert_eq!(
            *thread_description,
            "Request the verified source thread; native Lark may start the exact admitted\ngroup root thread with one text reply when explicitly advertised. Never fall back."
        );
        *thread_description = previous["inputSchema"]["$defs"]["OutboundMessage"]["properties"]
            ["thread_reply"]["description"]
            .clone();
        let key = tool["inputSchema"]["$defs"]["ReplyArguments"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("idempotency_key")
            .unwrap();
        assert_eq!(key["minLength"], 1);
        assert_eq!(key["maxLength"], 128);
        assert_eq!(key["pattern"], "^[!-~]+$");
        assert_native_media_update_extension(tool, previous, &key);
        let selector = tool["inputSchema"]["$defs"]["ReplyArguments"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("form_preparation_ref")
            .unwrap();
        assert!(selector.to_string().contains("string"));
        if tool["name"] == "replyToEvent" {
            assert_eq!(
                tool["inputSchema"]["properties"]
                    .as_object_mut()
                    .unwrap()
                    .remove("form_preparation_ref")
                    .unwrap(),
                selector
            );
        }

        if tool["name"] == "replyToEvent" {
            let direct = tool["inputSchema"]["properties"]
                .as_object_mut()
                .unwrap()
                .remove("idempotency_key")
                .unwrap();
            assert_eq!(direct, key);
            assert_eq!(
                tool["inputSchema"]["required"],
                json!(["event_ref", "message"])
            );
            let description = tool["description"].as_str().unwrap();
            for phrase in [
                "keep the same key and message",
                "new key for a new send",
                "different content is refused",
                "Legacy events",
            ] {
                assert!(description.contains(phrase));
            }
            assert_eq!(tool["annotations"]["idempotentHint"], false);
            tool["description"] = previous["description"].clone();
        }
    }
    assert_eq!(*list, golden);
}

fn assert_native_media_update_extension(tool: &mut Value, previous: &Value, reply_key: &Value) {
    let definitions = tool["inputSchema"]["$defs"].as_object_mut().unwrap();
    let patch = definitions.remove("NativeMediaUpdate").unwrap();
    assert_eq!(
        patch,
        json!({
            "additionalProperties": false,
            "properties": {
                "caption": {"description": "Telegram visible caption; Discord attachment description (alt text).", "type": "string"},
                "replacement": {"$ref": "#/$defs/NativeMediaReplacement"},
                "text": {"description": "Discord visible message content. Unsupported for Telegram media updates.", "type": "string"}
            },
            "type": "object"
        })
    );
    let replacement = definitions.remove("NativeMediaReplacement").unwrap();
    let mut expected = previous["inputSchema"]["$defs"]["NativeAttachment"].clone();
    expected["properties"]
        .as_object_mut()
        .unwrap()
        .remove("caption");
    assert_eq!(replacement, expected);
    let field = definitions["OutboundMessage"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("native_media_update")
        .unwrap();
    assert_eq!(
        field,
        json!({
            "$ref": "#/$defs/NativeMediaUpdate",
            "description": "Explicit single-media update; accepted only by updateMessage."
        })
    );
    let key = definitions["UpdateArguments"]["properties"]
        .as_object_mut()
        .unwrap()
        .remove("idempotency_key")
        .unwrap();
    let mut expected_key = reply_key.clone();
    expected_key["description"] =
        json!("Required only for native_media_update. Omitted for compatible legacy updates.");
    assert_eq!(key, expected_key);
    if tool["name"] == "updateMessage" {
        let direct = tool["inputSchema"]["properties"]
            .as_object_mut()
            .unwrap()
            .remove("idempotency_key")
            .unwrap();
        assert_eq!(direct, key);
        assert_eq!(tool["annotations"]["idempotentHint"], false);
        let description = tool["description"].as_str().unwrap();
        for phrase in [
            "explicit media-edit reference authority",
            "keep the same key and patch on retry",
            "never change keys after an unknown outcome",
            "Caption means visible Telegram caption or Discord attachment alt text",
            "text means visible Discord content",
        ] {
            assert!(description.contains(phrase));
        }
        tool["description"] = previous["description"].clone();
    }
}

#[tokio::test]
async fn mcp_migration_accepts_exact_canonical_and_legacy_origins() {
    mcp_advertisement_origins(false).await;
    mcp_advertisement_origins(true).await;
}

async fn mcp_advertisement_origins(advertise: bool) {
    let app = App::new().await;
    let mut state = app.state.clone();
    state.public_api_url = "https://bot.example/event-gateway".into();
    state.advertise_public_api_url = advertise;
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
