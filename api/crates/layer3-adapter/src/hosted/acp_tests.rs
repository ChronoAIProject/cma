use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::{
    ContentBlock, ContentChunk, ConversationInput, PermissionDecision, PermissionOption,
    PermissionOptionKind, PromptResponse, RequestPermissionRequest, SessionUpdate, ToolCallUpdate,
    ToolCallUpdateFields, TurnStopReason,
};

type ServerSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

#[path = "acp_channel_tool_tests.rs"]
mod channel_tool_tests;
#[path = "acp_provider_tool_tests.rs"]
mod provider_tool_tests;

#[path = "acp_reconnect_turn_tests.rs"]
mod reconnect_turn_tests;

#[path = "acp_output_tests.rs"]
mod output_tests;
#[path = "acp_settings_tests.rs"]
mod settings_tests;

async fn socket_pair() -> (Socket, ServerSocket) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0")
        .await
        .expect("WebSocket fixture binds");
    let address = listener.local_addr().expect("WebSocket fixture address");
    let server = async move {
        let (stream, _) = listener.accept().await.expect("WebSocket fixture accepts");
        tokio_tungstenite::accept_async(stream)
            .await
            .expect("server handshake succeeds")
    };
    let client = tokio_tungstenite::connect_async(format!("ws://{address}/conversation"));
    let (client, server) = tokio::join!(client, server);
    (client.expect("client handshake succeeds").0, server)
}

async fn send_ready(server: &mut ServerSocket, agent_id: &str) {
    server
        .send(Message::Text(
            serde_json::json!({
                "t": "ready",
                "wireVersion": 1,
                "lanes": ["ctl", "conv"],
                "agentId": agent_id,
            })
            .to_string(),
        ))
        .await
        .expect("Ready sends");
}

async fn receive_json(server: &mut ServerSocket) -> serde_json::Value {
    let frame = tokio::time::timeout(Duration::from_secs(2), server.next())
        .await
        .expect("client frame arrives")
        .expect("client socket remains open")
        .expect("client frame is readable");
    let Message::Binary(frame) = frame else {
        panic!("expected binary ACP frame, received {frame:?}");
    };
    assert_eq!(frame.first(), Some(&LANE_CONVERSATION));
    serde_json::from_slice(&frame[1..]).expect("client ACP frame is JSON")
}

async fn send_json(server: &mut ServerSocket, value: serde_json::Value) {
    let mut frame = vec![LANE_CONVERSATION];
    frame.extend(serde_json::to_vec(&value).expect("server ACP frame serializes"));
    server
        .send(Message::Binary(frame))
        .await
        .expect("server ACP frame sends");
}

async fn respond(
    server: &mut ServerSocket,
    request: &serde_json::Value,
    result: serde_json::Value,
) {
    send_json(
        server,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": request["id"].clone(),
            "result": result,
        }),
    )
    .await;
}

async fn reject(server: &mut ServerSocket, request: &serde_json::Value) {
    send_json(
        server,
        serde_json::json!({
            "jsonrpc": "2.0",
            "id": request["id"].clone(),
            "error": { "code": -32000, "message": "already initialized" },
        }),
    )
    .await;
}

fn settings(permission_mode: Option<&str>, own_credential: bool) -> HostedAgentOpenSettings {
    HostedAgentOpenSettings {
        permission_mode: permission_mode.map(str::to_string),
        own_credential,
        channel_tool: None,
        provider_tool: None,
        recovered_prompt: None,
    }
}

async fn stop(opened: OpenedBridge) {
    opened.bridge.retire().await;
    opened.reader.abort();
    opened.relay.abort();
    let _ = opened.reader.await;
    let _ = opened.relay.await;
}

#[test]
fn ready_declaration_requires_exact_version_agent_and_lanes() {
    let valid = serde_json::json!({
        "t": "ready",
        "wireVersion": 1,
        "lanes": ["ctl", "conv"],
        "agentId": "agent-1",
    });
    validate_ready(Message::Text(valid.to_string()), "agent-1").expect("exact Ready is accepted");

    let invalid = [
        serde_json::json!({
            "t": "not-ready", "wireVersion": 1,
            "lanes": ["ctl", "conv"], "agentId": "agent-1",
        }),
        serde_json::json!({
            "t": "ready", "wireVersion": 2,
            "lanes": ["ctl", "conv"], "agentId": "agent-1",
        }),
        serde_json::json!({
            "t": "ready", "wireVersion": 1,
            "lanes": ["conv"], "agentId": "agent-1",
        }),
        serde_json::json!({
            "t": "ready", "wireVersion": 1,
            "lanes": ["ctl", "conv"], "agentId": "agent-2",
        }),
    ];
    for declaration in invalid {
        assert!(validate_ready(Message::Text(declaration.to_string()), "agent-1").is_err());
    }
    assert!(validate_ready(Message::Binary(vec![1, 2, 3]), "agent-1").is_err());
}

#[tokio::test]
async fn fresh_bridge_routes_startup_prompt_permission_cancel_and_gap_frames() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        assert_eq!(initialize["method"], "initialize");
        assert_eq!(initialize["params"]["protocolVersion"], 1);
        assert_eq!(
            initialize.pointer("/params/clientCapabilities/_meta/jetbrains/air/capabilities"),
            Some(&serde_json::json!(["sessionFailure"]))
        );

        let startup = SessionUpdate::AgentMessageChunk(ContentChunk::new(ContentBlock::from(
            "startup activity",
        )));
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0",
                "method": "session/update",
                "params": { "sessionId": "conversation-1", "update": startup },
            }),
        )
        .await;
        respond(&mut server, &initialize, serde_json::json!({})).await;

        let new_session = receive_json(&mut server).await;
        assert_eq!(new_session["method"], "session/new");
        assert_eq!(new_session["params"]["mcpServers"], serde_json::json!([]));
        assert_eq!(new_session["params"]["cwd"], "/workspace");
        respond(
            &mut server,
            &new_session,
            serde_json::json!({ "sessionId": "conversation-1" }),
        )
        .await;

        let mode = receive_json(&mut server).await;
        assert_eq!(mode["method"], "session/set_mode");
        assert_eq!(mode["params"]["sessionId"], "conversation-1");
        assert_eq!(mode["params"]["modeId"], "read-only");
        respond(&mut server, &mode, serde_json::json!({})).await;

        let prompt = receive_json(&mut server).await;
        assert_eq!(prompt["method"], "session/prompt");
        assert_eq!(prompt["params"]["sessionId"], "conversation-1");

        let options = vec![
            PermissionOption::new("allow", "Allow", PermissionOptionKind::AllowOnce),
            PermissionOption::new("deny", "Deny", PermissionOptionKind::RejectOnce),
        ];
        let wrong_session = RequestPermissionRequest::new(
            "conversation-other",
            ToolCallUpdate::new("tool-1", ToolCallUpdateFields::default()),
            options.clone(),
        );
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 70,
                "method": "session/request_permission", "params": wrong_session,
            }),
        )
        .await;
        let refused = receive_json(&mut server).await;
        assert_eq!(refused["id"], 70);
        assert_eq!(refused["error"]["code"], -32602);

        let permission = RequestPermissionRequest::new(
            "conversation-1",
            ToolCallUpdate::new("tool-1", ToolCallUpdateFields::default()),
            options,
        );
        send_json(
            &mut server,
            serde_json::json!({
                "jsonrpc": "2.0", "id": 71,
                "method": "session/request_permission", "params": permission,
            }),
        )
        .await;
        let first = receive_json(&mut server).await;
        let repeated = receive_json(&mut server).await;
        assert_eq!(first, repeated, "repeated durable answer is idempotent");
        assert_eq!(first["id"], 71);
        assert_eq!(first["result"]["outcome"]["outcome"], "selected");
        assert_eq!(first["result"]["outcome"]["optionId"], "allow");

        respond(
            &mut server,
            &prompt,
            serde_json::to_value(PromptResponse::new(TurnStopReason::EndTurn))
                .expect("prompt response serializes"),
        )
        .await;
        let mut gap = vec![LANE_CONVERSATION_GAP];
        gap.extend_from_slice(br#"{"frames":4}"#);
        server
            .send(Message::Binary(gap))
            .await
            .expect("gap frame sends");

        let cancel = receive_json(&mut server).await;
        assert_eq!(cancel["method"], "session/cancel");
        assert_eq!(cancel["params"]["sessionId"], "conversation-1");
    });

    let (updates_tx, mut updates) = mpsc::channel(16);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(Some("read-only"), false),
        None,
        updates_tx,
    )
    .await
    .expect("fresh bridge opens");
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation-1")
    );
    assert!(matches!(
        updates.recv().await.expect("startup update"),
        ConversationUpdate::WithoutPromptEvidence(update)
            if matches!(*update, ConversationUpdate::Said(_))
    ));

    opened
        .channel
        .input
        .send(ConversationInput::Prompt(vec![ContentBlock::from("hello")]))
        .await
        .expect("prompt writes");
    let asked = updates.recv().await.expect("permission update");
    let ConversationUpdate::Asked(asked) = asked else {
        panic!("expected permission update, received {asked:?}");
    };
    let locator = asked.locator.expect("permission carries durable locator");
    let answer = ConversationInput::PermissionAnswer {
        id: asked.id,
        locator: Some(locator),
        decision: PermissionDecision::Allow,
    };
    opened
        .channel
        .input
        .send(answer.clone())
        .await
        .expect("permission answer writes");
    opened
        .channel
        .input
        .send(answer)
        .await
        .expect("repeated permission answer writes");
    assert!(matches!(
        updates.recv().await.expect("turn result"),
        ConversationUpdate::TurnEnded(_)
    ));
    assert!(matches!(
        updates.recv().await.expect("gap update"),
        ConversationUpdate::Lost { frames: 4 }
    ));
    opened
        .channel
        .input
        .send(ConversationInput::Cancel)
        .await
        .expect("cancel writes");

    server.await.expect("ACP fixture completes");
    stop(opened).await;
}

#[tokio::test]
async fn fresh_resume_loads_the_exact_held_session() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (release, held) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let load = receive_json(&mut server).await;
        assert_eq!(load["method"], "session/load");
        assert_eq!(load["params"]["mcpServers"], serde_json::json!([]));
        assert_eq!(load["params"]["sessionId"], "conversation-held");
        assert_eq!(load["params"]["cwd"], "/workspace");
        respond(&mut server, &load, serde_json::json!({})).await;
        let _ = held.await;
    });
    let (updates, _) = mpsc::channel(4);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        Some("conversation-held".into()),
        updates,
    )
    .await
    .expect("held conversation resumes");
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation-held")
    );
    let _ = release.send(());
    server.await.expect("ACP fixture completes");
    stop(opened).await;
}

#[tokio::test]
async fn initialized_bridge_reuses_only_an_explicit_held_session() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
        assert!(
            tokio::time::timeout(Duration::from_millis(150), server.next())
                .await
                .is_err()
        );
    });
    let (updates, _) = mpsc::channel(4);
    let opened = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        Some("conversation-held".into()),
        updates,
    )
    .await
    .expect("initialized bridge reuses explicit session");
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation-held")
    );
    server.await.expect("ACP fixture completes");
    stop(opened).await;

    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        reject(&mut server, &initialize).await;
    });
    let (updates, _) = mpsc::channel(4);
    let error = match Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, false),
        None,
        updates,
    )
    .await
    {
        Ok(opened) => {
            stop(opened).await;
            panic!("initialized bridge without held identity was accepted");
        }
        Err(error) => error,
    };
    assert!(matches!(error, AdapterError::Rejected(detail)
        if detail.contains("held conversation identifier")));
    server.await.expect("ACP fixture completes");
}

#[tokio::test]
async fn owner_authentication_precedes_a_fresh_session() {
    let (client, mut server) = socket_pair().await;
    send_ready(&mut server, "agent-1").await;
    let (release, held) = tokio::sync::oneshot::channel();
    let (authorize, authorized) = tokio::sync::oneshot::channel();
    let server = tokio::spawn(async move {
        let initialize = receive_json(&mut server).await;
        respond(
            &mut server,
            &initialize,
            serde_json::json!({ "authMethods": [{ "id": "device-code" }] }),
        )
        .await;
        let authenticate = receive_json(&mut server).await;
        assert_eq!(authenticate["method"], "authenticate");
        assert_eq!(authenticate["params"]["methodId"], "device-code");
        send_json(&mut server, serde_json::json!({
            "jsonrpc": "2.0", "id": 91, "method": "elicitation/create",
            "params": { "url": "https://login.example.test/device", "userCode": "fixture-code" },
        })).await;
        let elicitation = receive_json(&mut server).await;
        assert_eq!(elicitation["id"], 91);
        assert_eq!(elicitation["result"]["action"], "accept");
        authorized
            .await
            .expect("caller receives and authorizes the device challenge");
        respond(&mut server, &authenticate, serde_json::json!({})).await;
        let new_session = receive_json(&mut server).await;
        assert_eq!(new_session["method"], "session/new");
        assert_eq!(new_session["params"]["mcpServers"], serde_json::json!([]));
        respond(
            &mut server,
            &new_session,
            serde_json::json!({ "sessionId": "conversation-owner" }),
        )
        .await;
        let _ = held.await;
    });
    let (updates, mut receiver) = mpsc::channel(4);
    let opening = Bridge::open(
        client,
        "agent-1",
        "/workspace".into(),
        settings(None, true),
        None,
        updates,
    );
    tokio::pin!(opening);
    tokio::select! {
        update = receiver.recv() => assert!(matches!(update, Some(ConversationUpdate::SignInRequired { code: Some(code), .. }) if code == "fixture-code")),
        _ = &mut opening => panic!("opening cannot finish before owner authentication"),
    }
    authorize.send(()).unwrap();
    let opened = opening.await.expect("owner bridge authenticates and opens");
    assert_eq!(
        opened.channel.conversation_id.as_deref(),
        Some("conversation-owner")
    );
    let _ = release.send(());
    server.await.expect("ACP fixture completes");
    stop(opened).await;
}
