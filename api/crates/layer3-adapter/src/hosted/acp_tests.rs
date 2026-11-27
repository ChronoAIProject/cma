use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use tokio_tungstenite::tungstenite::Message;

use super::*;
use crate::{
    ContentBlock, ContentChunk, ConversationInput, PermissionDecision, PermissionOption,
    PermissionOptionKind, PromptResponse, RequestPermissionRequest, SessionUpdate, ToolCallUpdate,
    ToolCallUpdateFields, TurnStopReason,
};

pub(super) type ServerSocket = tokio_tungstenite::WebSocketStream<tokio::net::TcpStream>;

#[path = "acp_permission_tests.rs"]
mod permission_tests;

#[path = "acp_access_tool_tests.rs"]
mod access_tool_tests;
#[path = "acp_channel_tool_tests.rs"]
mod channel_tool_tests;
#[path = "acp_initialize_tests.rs"]
mod initialize_tests;
#[path = "acp_open_completion_tests.rs"]
mod open_completion_tests;
#[path = "acp_provider_tool_tests.rs"]
mod provider_tool_tests;
#[path = "acp_startup_failure_tests.rs"]
mod startup_failure_tests;
#[path = "acp_startup_timing_tests.rs"]
mod startup_timing_tests;

#[path = "acp_buffered_recovery_tests.rs"]
mod buffered_recovery_tests;
#[path = "acp_fresh_bridge_tests.rs"]
mod fresh_bridge_tests;
#[path = "acp_provider_recovery_tests.rs"]
mod provider_recovery_tests;
#[path = "acp_reconnect_turn_tests.rs"]
mod reconnect_turn_tests;
#[path = "acp_recovery_deadline_tests.rs"]
mod recovery_deadline_tests;

#[path = "acp_backpressure_tests.rs"]
mod backpressure_tests;
#[path = "acp_context_refresh_tests.rs"]
mod context_refresh_tests;
#[path = "acp_output_tests.rs"]
mod output_tests;
#[path = "acp_settings_tests.rs"]
mod settings_tests;
#[path = "acp_steering_tests.rs"]
mod steering_tests;
#[path = "acp_transport_refresh_tests.rs"]
mod transport_refresh_tests;

pub(super) async fn socket_pair() -> (Socket, ServerSocket) {
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
    (
        Box::new(client.expect("client handshake succeeds").0),
        server,
    )
}

pub(in crate::hosted) async fn open_idle_fixture(
    agent: &str,
    updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
) -> (OpenedBridge, ServerSocket) {
    let (client, mut server) = socket_pair().await;
    let serve = async {
        send_ready(&mut server, agent).await;
        let initialize = receive_json(&mut server).await;
        respond(&mut server, &initialize, serde_json::json!({})).await;
        let session = receive_json(&mut server).await;
        respond(
            &mut server,
            &session,
            serde_json::json!({"sessionId": "file-read-fixture"}),
        )
        .await;
        server
    };
    let open = Bridge::open_with_relay_policy(
        client,
        agent,
        "/workspace".into(),
        (settings(None, false), None, (false, None)),
        None,
        None,
        updates,
    );
    let (opened, server) = tokio::join!(open, serve);
    (opened.unwrap(), server)
}

pub(super) async fn send_ready(server: &mut ServerSocket, agent_id: &str) {
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

pub(super) async fn receive_json(server: &mut ServerSocket) -> serde_json::Value {
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
            "error": {
                "code": -32603, "message": "Internal error",
                "data": { "details": "Already initialized" },
            },
        }),
    )
    .await;
}

pub(super) fn settings(
    permission_mode: Option<&str>,
    own_credential: bool,
) -> HostedAgentOpenSettings {
    HostedAgentOpenSettings {
        coordinated_ingress: false,
        retained_json: false,
        bounded_raw_output: false,
        bounded_permission_state: false,
        output_json_budget: false,
        shared_runtime_output: false,
        already_initialized_error: Some(serde_json::json!({
            "code": -32603, "message": "Internal error",
            "data": { "details": "Already initialized" },
        })),
        open_completion: None,
        context_refresh: None,
        permission_mode: permission_mode.map(str::to_string),
        own_credential,
        channel_tool: None,
        provider_tool: None,
        access_tool: None,
        recovered_prompt: None,
        recovered_context_sequence: None,
    }
}

pub(super) async fn stop<C, B: std::ops::Deref<Target = Bridge>>(opened: OpenedBridge<C, B>) {
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
    validate_ready(Message::Text(valid.to_string()), "agent-1", false, None)
        .expect("exact Ready is accepted");

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
        assert!(validate_ready(
            Message::Text(declaration.to_string()),
            "agent-1",
            false,
            None
        )
        .is_err());
    }
    assert!(validate_ready(Message::Binary(vec![1, 2, 3]), "agent-1", false, None).is_err());
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

#[path = "acp_provider_refresh_tests.rs"]
mod provider_refresh_tests;
