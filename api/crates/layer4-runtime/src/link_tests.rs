use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use layer3_adapter::wire::{FencedRequest, RequestFrame, ResponseFrame, WireOperation};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentLaunch, AgentSlot, CheckpointRequest, CredentialFrame,
    InstallRequest, RestoreRequest, RuntimeCredential, WorkspaceRequest, WorkspaceSource,
    WorktreeRequest,
};
use tokio_tungstenite::tungstenite::Message as ClientMessage;

use super::*;
use crate::lanes;

#[path = "link_agent_resources_tests.rs"]
mod agent_resources;
#[path = "link_workspace_recovery_tests.rs"]
mod workspace_recovery;

fn add_worktree_request(request: WorktreeRequest) -> RequestFrame {
    RequestFrame::AddWorktree(FencedRequest::new(1, request))
}

fn materialize_home_request(request: AgentHomePlan) -> RequestFrame {
    RequestFrame::MaterializeAgentHome(FencedRequest::new(1, request))
}

fn open_agent_request(request: AgentLaunch) -> RequestFrame {
    RequestFrame::OpenAgent(FencedRequest::new(1, request))
}

fn close_agent_request(request: AgentCloseRequest) -> RequestFrame {
    RequestFrame::CloseAgent(FencedRequest::new(1, request))
}

fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cma-link-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
    let _ = std::fs::remove_dir_all(root.with_extension("checkpoints"));
    let _ = std::fs::remove_dir_all(root.with_extension("runtime-state"));
    root
}

fn supervisor(home: PathBuf) -> Arc<Supervisor> {
    Supervisor::new(
        crate::config::RuntimeConfig::for_test(home),
        crate::writer_fence::SupervisorBoundary::default(),
    )
    .unwrap()
}

async fn serve(supervisor: Arc<Supervisor>) -> (SocketAddr, tokio::task::JoinHandle<()>) {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move {
        axum::serve(listener, router(supervisor)).await.unwrap();
    });
    (address, server)
}

#[tokio::test]
async fn desktop_route_is_typed_404_for_a_non_machine_runtime() {
    let home = root("desktop-unavailable");
    let supervisor = supervisor(home.clone());
    let (address, server) = serve(supervisor).await;
    let error = tokio_tungstenite::connect_async(format!("ws://{address}/runtime/desktop"))
        .await
        .expect_err("non-machine runtimes must reject desktop upgrades");
    match error {
        tokio_tungstenite::tungstenite::Error::Http(response) => {
            assert_eq!(response.status(), StatusCode::NOT_FOUND);
        }
        other => panic!("unexpected desktop upgrade error: {other:?}"),
    }
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

async fn exchange(
    client: &reqwest::Client,
    address: SocketAddr,
    frame: &RequestFrame,
) -> ResponseFrame {
    let response = client
        .post(format!("http://{address}/runtime/control"))
        .json(frame)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    response.json().await.unwrap()
}

fn credential() -> RequestFrame {
    RequestFrame::Credential(CredentialFrame {
        operation_id: "credential-1".into(),
        value: RuntimeCredential::new("secret"),
        custody_generation: Some(1),
        github_route: None,
    })
}

async fn install_and_credential(supervisor: &Supervisor) {
    let installed = supervisor
        .dispatch(RequestFrame::Install(InstallRequest::bounded(
            "install-link",
            Vec::new(),
            5,
            1024,
        )))
        .await;
    assert!(
        matches!(installed, ResponseFrame::Install(_)),
        "{installed:?}"
    );
    let credentialed = supervisor.dispatch(credential()).await;
    assert!(
        matches!(credentialed, ResponseFrame::Credential(_)),
        "{credentialed:?}"
    );
}

#[tokio::test]
async fn shared_control_frames_round_trip_over_http() {
    let home = root("control");
    let supervisor = supervisor(home.clone());
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let client = reqwest::Client::new();

    let installed = exchange(
        &client,
        address,
        &RequestFrame::Install(InstallRequest::bounded(
            "install-frame",
            Vec::new(),
            5,
            1024,
        )),
    )
    .await;
    assert!(matches!(installed, ResponseFrame::Install(_)));
    let credentialed = exchange(&client, address, &credential()).await;
    assert!(matches!(credentialed, ResponseFrame::Credential(_)));
    let resources = exchange(&client, address, &RequestFrame::Resources).await;
    assert!(matches!(resources, ResponseFrame::Resources(_)));
    assert_eq!(supervisor.phase().await, Phase::Credentialed);

    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn unknown_tag_is_http_400_and_cannot_mutate_lifecycle() {
    let home = root("unknown");
    let supervisor = supervisor(home.clone());
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/runtime/control"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"t":"install_and_credential","commands":[]}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::BAD_REQUEST);
    let body: serde_json::Value = response.json().await.unwrap();
    assert_eq!(body["t"], "failure");
    assert_eq!(body["code"], "unknown_frame");
    assert_eq!(supervisor.phase().await, Phase::Provisioned);

    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn malformed_known_tag_returns_failure_for_that_operation() {
    let home = root("malformed");
    let supervisor = supervisor(home.clone());
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let response = reqwest::Client::new()
        .post(format!("http://{address}/runtime/control"))
        .header(header::CONTENT_TYPE, "application/json")
        .body(r#"{"t":"add_worktree","workspace":"project"}"#)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let frame: ResponseFrame = response.json().await.unwrap();
    let failure = frame.failure().expect("known malformed frame is typed");
    assert_eq!(failure.operation, WireOperation::AddWorktree);
    assert_eq!(failure.code, "malformed_frame");
    assert_eq!(supervisor.phase().await, Phase::Provisioned);

    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

async fn echo_supervisor(name: &str) -> (PathBuf, Arc<Supervisor>) {
    let home = root(name);
    let supervisor = supervisor(home.clone());
    install_and_credential(&supervisor).await;
    let workspace = match supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_link:prepare:v1",
            "wks_link",
            "project",
            WorkspaceSource::Init,
        )))
        .await
    {
        ResponseFrame::PrepareWorkspace(receipt) => receipt,
        other => panic!("workspace preparation failed: {other:?}"),
    };
    let worktree = match supervisor
        .dispatch(add_worktree_request(WorktreeRequest::new(
            "worktree:project:agent-1:add:v1",
            "project",
            "agent-1",
            workspace.resolved_commit,
        )))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt,
        other => panic!("worktree addition failed: {other:?}"),
    };
    let materialized = supervisor
        .dispatch(materialize_home_request(AgentHomePlan {
            operation_id: "agent:agent-1:home:create:v1".into(),
            request_sha256: "a".repeat(64),
            agent_id: AgentSlot::new("agent-1"),
            files: Vec::new(),
            symlinks: Vec::new(),
            skills: Vec::new(),
        }))
        .await;
    assert!(matches!(
        materialized,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let agent_home = crate::config::RuntimeConfig::for_test(home.clone()).agent_home("agent-1");
    let opened = supervisor
        .dispatch(open_agent_request(AgentLaunch {
            operation_id: "agent:agent-1:open:v1".into(),
            request_sha256: "b".repeat(64),
            agent_id: AgentSlot::new("agent-1"),
            cwd: worktree.receipt.path,
            home: agent_home.to_string_lossy().into_owned(),
            program: "/bin/sh".into(),
            args: vec![
                "-c".into(),
                "while IFS= read -r line; do printf '%s\\n' \"$line\"; done".into(),
            ],
            env: BTreeMap::new(),
        }))
        .await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    (home, supervisor)
}

#[tokio::test]
async fn websocket_announces_and_relays_the_opaque_conversation_lane() {
    let (home, supervisor) = echo_supervisor("websocket").await;
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/runtime/agents/agent-1/conversation"
    ))
    .await
    .unwrap();
    let ready = tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .unwrap()
        .unwrap()
        .unwrap();
    let ready: serde_json::Value = serde_json::from_str(&ready.into_text().unwrap()).unwrap();
    assert_eq!(ready["t"], "ready");
    assert_eq!(ready["wireVersion"], lanes::WIRE_VERSION);
    assert_eq!(ready["agentId"], "agent-1");
    assert!(ready.get("outputAcknowledgement").is_none());

    let payload = br#"{"jsonrpc":"2.0","id":7}"#;
    socket
        .send(ClientMessage::Binary(lanes::frame(
            lanes::CONVERSATION,
            payload,
        )))
        .await
        .unwrap();
    let echoed = tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            match socket.next().await.unwrap().unwrap() {
                ClientMessage::Binary(bytes) => break bytes,
                ClientMessage::Close(_) => panic!("conversation closed before echo"),
                _ => {}
            }
        }
    })
    .await
    .expect("agent echoed the conversation frame");
    assert_eq!(echoed, lanes::frame(lanes::CONVERSATION, payload));
    socket.close(None).await.unwrap();
    let bridge = supervisor.bridge("agent-1").await.unwrap();
    let retained = bridge.retained_output.snapshot(0).await.unwrap();
    // Legacy delivery has no durable ACK and must not fill the retained window.
    assert!(retained.frames.is_empty());
    let closed = supervisor
        .dispatch(close_agent_request(AgentCloseRequest::new(
            "agent:agent-1:close:v1",
            AgentSlot::new("agent-1"),
        )))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)));

    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

type ClientSocket =
    tokio_tungstenite::WebSocketStream<tokio_tungstenite::MaybeTlsStream<tokio::net::TcpStream>>;

async fn reliable_socket(address: SocketAddr) -> (ClientSocket, uuid::Uuid) {
    let (mut socket, _) = tokio_tungstenite::connect_async(format!(
        "ws://{address}/runtime/agents/agent-1/conversation?outputAck=1"
    ))
    .await
    .unwrap();
    let ready = next_conversation_message(&mut socket).await;
    let ready: serde_json::Value = serde_json::from_str(&ready.into_text().unwrap()).unwrap();
    assert_eq!(ready["wireVersion"], 1);
    assert_eq!(ready["outputAcknowledgement"]["version"], 1);
    let stream =
        uuid::Uuid::parse_str(ready["outputAcknowledgement"]["streamId"].as_str().unwrap())
            .unwrap();
    (socket, stream)
}

async fn next_conversation_message(socket: &mut ClientSocket) -> ClientMessage {
    tokio::time::timeout(Duration::from_secs(2), socket.next())
        .await
        .expect("conversation message deadline")
        .unwrap()
        .unwrap()
}

#[tokio::test]
async fn legacy_first_bridge_stays_legacy_when_a_new_api_requests_acknowledgements() {
    let (home, supervisor) = echo_supervisor("legacy-first").await;
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let endpoint = format!("ws://{address}/runtime/agents/agent-1/conversation");
    let (mut legacy, _) = tokio_tungstenite::connect_async(endpoint.as_str())
        .await
        .unwrap();
    assert!(matches!(
        next_conversation_message(&mut legacy).await,
        ClientMessage::Text(_)
    ));
    legacy
        .send(ClientMessage::Binary(lanes::frame(
            lanes::CONVERSATION,
            b"already recorded",
        )))
        .await
        .unwrap();
    assert_eq!(
        next_conversation_message(&mut legacy).await,
        ClientMessage::Binary(lanes::frame(lanes::CONVERSATION, b"already recorded"))
    );
    legacy.close(None).await.unwrap();

    let (mut upgraded_api, _) = tokio_tungstenite::connect_async(format!("{endpoint}?outputAck=1"))
        .await
        .unwrap();
    let ready = next_conversation_message(&mut upgraded_api)
        .await
        .into_text()
        .unwrap();
    let ready: serde_json::Value = serde_json::from_str(&ready).unwrap();
    assert!(ready.get("outputAcknowledgement").is_none());
    upgraded_api
        .send(ClientMessage::Binary(lanes::frame(
            lanes::CONVERSATION,
            b"next turn",
        )))
        .await
        .unwrap();
    // The legacy output is absent; the first native output belongs to this request.
    assert_eq!(
        next_conversation_message(&mut upgraded_api).await,
        ClientMessage::Binary(lanes::frame(lanes::CONVERSATION, b"next turn"))
    );
    upgraded_api.close(None).await.unwrap();
    supervisor
        .bridge("agent-1")
        .await
        .unwrap()
        .terminate()
        .await
        .unwrap();
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn reliable_first_bridge_rejects_a_legacy_downgrade_before_websocket_upgrade() {
    let (home, supervisor) = echo_supervisor("reliable-first").await;
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let (mut reliable, _) = reliable_socket(address).await;
    reliable.close(None).await.unwrap();
    let result = tokio_tungstenite::connect_async(format!(
        "ws://{address}/runtime/agents/agent-1/conversation"
    ))
    .await;
    let tokio_tungstenite::tungstenite::Error::Http(response) = result.unwrap_err() else {
        panic!("legacy downgrade must fail before WebSocket upgrade");
    };
    assert_eq!(response.status().as_u16(), StatusCode::CONFLICT.as_u16());
    let body: serde_json::Value =
        serde_json::from_slice(response.body().as_deref().unwrap()).unwrap();
    assert_eq!(body["code"], "output_acknowledgement_required");
    supervisor
        .bridge("agent-1")
        .await
        .unwrap()
        .terminate()
        .await
        .unwrap();
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn reliable_websocket_replays_terminal_after_disconnect_and_releases_only_after_ack() {
    use sha2::Digest as _;

    let (home, supervisor) = echo_supervisor("reliable-replay").await;
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let (mut first, stream) = reliable_socket(address).await;
    let terminal = br#"{"jsonrpc":"2.0","id":"prompt-1","result":{"stopReason":"end_turn"}}"#;
    first
        .send(ClientMessage::Binary(lanes::frame(
            lanes::CONVERSATION,
            terminal,
        )))
        .await
        .unwrap();
    let first_terminal = next_conversation_message(&mut first).await;
    assert_eq!(
        first_terminal,
        ClientMessage::Binary(lanes::acknowledged_frame(stream, 1, terminal))
    );
    first.close(None).await.unwrap();

    let (mut replacement, replacement_stream) = reliable_socket(address).await;
    assert_eq!(replacement_stream, stream);
    assert_eq!(
        next_conversation_message(&mut replacement).await,
        first_terminal
    );
    let mut acknowledgement = lanes::acknowledged_frame(stream, 1, b"");
    acknowledgement[0] = lanes::OUTPUT_ACKNOWLEDGEMENT;
    replacement
        .send(ClientMessage::Binary(acknowledgement))
        .await
        .unwrap();
    replacement
        .send(ClientMessage::Binary(lanes::frame(
            lanes::CONVERSATION,
            b"next",
        )))
        .await
        .unwrap();
    assert_eq!(
        next_conversation_message(&mut replacement).await,
        ClientMessage::Binary(lanes::acknowledged_frame(stream, 2, b"next"))
    );
    let retained = supervisor.bridge("agent-1").await.unwrap();
    let evidence = retained.retained_output.recovery_evidence().await.unwrap();
    assert_eq!(
        (
            evidence.boundary.stream_id,
            evidence.boundary.acknowledged_through,
            evidence.acknowledged_payload_sha256,
        ),
        (
            stream.to_string(),
            1,
            Some(sha2::Sha256::digest(terminal).into())
        )
    );
    replacement.close(None).await.unwrap();

    let (mut third, _) = reliable_socket(address).await;
    assert_eq!(
        next_conversation_message(&mut third).await,
        ClientMessage::Binary(lanes::acknowledged_frame(stream, 2, b"next"))
    );
    third.close(None).await.unwrap();
    supervisor
        .bridge("agent-1")
        .await
        .unwrap()
        .terminate()
        .await
        .unwrap();
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn reliable_terminal_remains_reachable_after_native_exit_until_final_ack() {
    let (home, supervisor) = echo_supervisor("reliable-exited").await;
    let bridge = supervisor.bridge("agent-1").await.unwrap();
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let (mut first, stream) = reliable_socket(address).await;
    let terminal = br#"{"jsonrpc":"2.0","id":"prompt-1","result":{"stopReason":"end_turn"}}"#;
    first
        .send(ClientMessage::Binary(lanes::frame(
            lanes::CONVERSATION,
            terminal,
        )))
        .await
        .unwrap();
    let expected = next_conversation_message(&mut first).await;
    first.close(None).await.unwrap();
    bridge.terminate().await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), bridge.ended())
        .await
        .unwrap();
    assert!(Arc::ptr_eq(
        &bridge,
        &supervisor.bridge("agent-1").await.unwrap()
    ));

    let (mut replacement, replay_stream) = reliable_socket(address).await;
    assert_eq!(replay_stream, stream);
    assert_eq!(next_conversation_message(&mut replacement).await, expected);
    let mut acknowledgement = lanes::acknowledged_frame(stream, 1, b"");
    acknowledgement[0] = lanes::OUTPUT_ACKNOWLEDGEMENT;
    replacement
        .send(ClientMessage::Binary(acknowledgement))
        .await
        .unwrap();
    assert!(matches!(
        next_conversation_message(&mut replacement).await,
        ClientMessage::Close(_)
    ));
    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn reliable_backpressure_delivers_both_frames_and_releases_only_after_ack() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (home, supervisor) = echo_supervisor("reliable-backpressure").await;
        let bridge = supervisor.bridge("agent-1").await.unwrap();
        let first = vec![b'x'; crate::bridge::MAX_FRAME_BYTES];
        let second = vec![b'y'; crate::bridge::MAX_FRAME_BYTES];
        bridge.retained_output.retain(&first).await;
        let retaining = bridge.retained_output.retain(&second);
        tokio::pin!(retaining);
        // Frame metadata makes two maximum-size payloads exceed the byte window.
        // Keep this producer pending until the real socket consumer ACKs.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut retaining)
                .await
                .is_err()
        );
        let (address, server) = serve(Arc::clone(&supervisor)).await;
        let (mut socket, stream) = reliable_socket(address).await;
        assert!(
            next_conversation_message(&mut socket).await
                == ClientMessage::Binary(lanes::acknowledged_frame(stream, 1, &first)),
            "the entire first frame must arrive with its original cursor"
        );
        // Sending the frame over the socket does not release durable ownership.
        assert!(
            tokio::time::timeout(Duration::from_millis(30), &mut retaining)
                .await
                .is_err()
        );
        let mut acknowledgement = lanes::acknowledged_frame(stream, 1, b"");
        acknowledgement[0] = lanes::OUTPUT_ACKNOWLEDGEMENT;
        socket
            .send(ClientMessage::Binary(acknowledgement))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), &mut retaining)
            .await
            .expect("the first ACK must release the blocked producer");
        assert!(
            next_conversation_message(&mut socket).await
                == ClientMessage::Binary(lanes::acknowledged_frame(stream, 2, &second)),
            "the entire second frame must arrive next without a gap"
        );
        let mut changed = bridge.retained_output.subscribe();
        let mut acknowledgement = lanes::acknowledged_frame(stream, 2, b"");
        acknowledgement[0] = lanes::OUTPUT_ACKNOWLEDGEMENT;
        socket
            .send(ClientMessage::Binary(acknowledgement))
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), async {
            while !bridge
                .retained_output
                .snapshot(0)
                .await
                .unwrap()
                .frames
                .is_empty()
            {
                changed.changed().await.unwrap();
            }
        })
        .await
        .expect("both acknowledged frames must leave the retained window");
        assert!(!bridge.is_ended());
        socket
            .send(ClientMessage::Binary(lanes::frame(
                lanes::CONVERSATION,
                b"still running",
            )))
            .await
            .unwrap();
        assert_eq!(
            next_conversation_message(&mut socket).await,
            ClientMessage::Binary(lanes::acknowledged_frame(stream, 3, b"still running"))
        );
        socket.close(None).await.unwrap();
        bridge.terminate().await.unwrap();
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(home).unwrap();
    })
    .await
    .expect("reliable backpressure test deadline");
}

#[tokio::test]
async fn reliable_explicit_gap_closes_without_terminating_native_process() {
    tokio::time::timeout(Duration::from_secs(15), async {
        let (home, supervisor) = echo_supervisor("reliable-gap").await;
        let bridge = supervisor.bridge("agent-1").await.unwrap();
        bridge.retained_output.gap().await;
        let (address, server) = serve(Arc::clone(&supervisor)).await;
        let (mut socket, _) = reliable_socket(address).await;
        let gap = next_conversation_message(&mut socket).await.into_data();
        let (lane, payload) = lanes::unframe(&gap).unwrap();
        assert_eq!(lane, lanes::CONVERSATION_GAP);
        assert_eq!(
            serde_json::from_slice::<serde_json::Value>(payload).unwrap()["frames"],
            1
        );
        assert!(matches!(
            next_conversation_message(&mut socket).await,
            ClientMessage::Close(_)
        ));
        assert!(!bridge.is_ended());

        let mut native = bridge.subscribe();
        assert!(bridge.send(b"still running".to_vec()).await);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(2), native.recv())
                .await
                .unwrap()
                .unwrap(),
            crate::bridge::Outbound::Frame(b"still running".to_vec())
        );
        bridge.terminate().await.unwrap();
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(home).unwrap();
    })
    .await
    .expect("reliable explicit gap test deadline");
}

#[tokio::test]
async fn checkpoint_objects_stream_between_runtime_stores() {
    let source_home = root("checkpoint-source");
    let source = supervisor(source_home.clone());
    std::fs::write(source_home.join("preserved"), "checkpoint payload").unwrap();
    let receipt = source
        .checkpoint_store()
        .capture(&CheckpointRequest {
            epoch: 9,
            exclusions: Vec::new(),
        })
        .await
        .unwrap();
    let (source_address, source_server) = serve(source).await;
    let client = reqwest::Client::new();
    let download = client
        .get(format!(
            "http://{source_address}/runtime/checkpoints/{}",
            receipt.object_reference
        ))
        .send()
        .await
        .unwrap();
    assert_eq!(download.status(), StatusCode::OK);
    assert_eq!(
        download.headers()[header::CONTENT_TYPE],
        "application/octet-stream"
    );
    let archive = download.bytes().await.unwrap();

    let target_home = root("checkpoint-target");
    let target = supervisor(target_home.clone());
    let (target_address, target_server) = serve(Arc::clone(&target)).await;
    let uploaded = client
        .put(format!(
            "http://{target_address}/runtime/checkpoints/{}",
            receipt.object_reference
        ))
        .body(archive.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(uploaded.status(), StatusCode::NO_CONTENT);
    let committed = client
        .put(format!(
            "http://{target_address}/runtime/checkpoints/{}",
            receipt.object_reference
        ))
        .header(header::ACCEPT, CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE)
        .body(archive.clone())
        .send()
        .await
        .unwrap();
    assert_eq!(committed.status(), StatusCode::OK);
    assert_eq!(
        committed.headers()[header::CONTENT_TYPE],
        CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE
    );
    assert_eq!(
        committed.json::<CheckpointUploadResponse>().await.unwrap(),
        CheckpointUploadResponse::Committed {
            object_reference: receipt.object_reference.clone(),
            sha256: receipt.sha256.clone(),
            size_bytes: archive.len() as u64,
        }
    );
    let downloaded = client
        .get(format!(
            "http://{target_address}/runtime/checkpoints/{}",
            receipt.object_reference
        ))
        .send()
        .await
        .unwrap()
        .bytes()
        .await
        .unwrap();
    assert_eq!(downloaded, archive);
    target
        .checkpoint_store()
        .restore(&RestoreRequest {
            object_reference: receipt.object_reference,
            sha256: receipt.sha256,
        })
        .await
        .unwrap();
    assert_eq!(
        std::fs::read_to_string(target_home.join("preserved")).unwrap(),
        "checkpoint payload"
    );

    source_server.abort();
    target_server.abort();
    let _ = source_server.await;
    let _ = target_server.await;
    let _ = std::fs::remove_dir_all(source_home.with_extension("checkpoints"));
    let _ = std::fs::remove_dir_all(target_home.with_extension("checkpoints"));
    std::fs::remove_dir_all(source_home).unwrap();
    std::fs::remove_dir_all(target_home).unwrap();
}

#[tokio::test]
async fn checkpoint_upload_opt_in_reports_invalid_archive_without_publication() {
    let home = root("invalid-streamed-upload");
    let supervisor = supervisor(home.clone());
    let (address, server) = serve(Arc::clone(&supervisor)).await;
    let bytes = b"not-a-checkpoint-archive";
    let sha256 = crate::command::hex(&Sha256::digest(bytes));
    let reference = format!("home-{sha256}.cma");
    let response = reqwest::Client::new()
        .put(format!("http://{address}/runtime/checkpoints/{reference}"))
        .header(header::ACCEPT, CHECKPOINT_UPLOAD_RECEIPT_MEDIA_TYPE)
        .body(bytes.as_slice())
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    assert!(matches!(
        response.json::<CheckpointUploadResponse>().await.unwrap(),
        CheckpointUploadResponse::Failure { code, retryable: false, .. }
            if code == "checkpoint_invalid"
    ));
    assert!(supervisor
        .checkpoint_store()
        .download_path(&reference)
        .is_err());
    assert!(!std::fs::read_dir(home.with_extension("checkpoints"))
        .unwrap()
        .any(|entry| entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".upload-")));
    server.abort();
    let _ = server.await;
    let _ = std::fs::remove_dir_all(home.with_extension("checkpoints"));
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test]
async fn checkpoint_heartbeat_keeps_json_readable_across_multiple_idle_boundaries() {
    let expected = serde_json::json!({"type": "committed", "size_bytes": 42});
    let completed = expected.clone();
    let (release, waiting) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        waiting.await.unwrap();
        completed
    });
    let response = heartbeat_json(task, Duration::from_millis(10));
    assert_eq!(response.headers()[header::CONTENT_TYPE], "application/json");
    assert!(!response.headers().contains_key(header::CONTENT_LENGTH));
    let mut stream = response.into_body().into_data_stream();
    let mut bytes = Vec::new();
    for _ in 0..3 {
        let chunk = tokio::time::timeout(Duration::from_millis(250), stream.next())
            .await
            .expect("each heartbeat arrives before the proxy read-idle boundary")
            .unwrap()
            .unwrap();
        assert_eq!(chunk.as_ref(), b"\n");
        bytes.extend_from_slice(&chunk);
    }
    release.send(()).unwrap();
    while let Some(chunk) = tokio::time::timeout(Duration::from_millis(250), stream.next())
        .await
        .expect("the completed JSON receipt terminates the stream")
    {
        let chunk = chunk.unwrap();
        bytes.extend_from_slice(&chunk);
    }
    assert_eq!(
        serde_json::from_slice::<serde_json::Value>(&bytes).unwrap(),
        expected
    );
}

#[tokio::test]
async fn checkpoint_heartbeat_disconnect_does_not_abort_validation_task() {
    let (release, waiting) = tokio::sync::oneshot::channel();
    let (completed, observed) = tokio::sync::oneshot::channel();
    let task = tokio::spawn(async move {
        waiting.await.unwrap();
        completed.send(()).unwrap();
        serde_json::json!({"type": "committed"})
    });
    let response = heartbeat_json(task, Duration::from_millis(10));
    let mut stream = response.into_body().into_data_stream();
    assert_eq!(stream.next().await.unwrap().unwrap().as_ref(), b"\n");
    drop(stream);
    release.send(()).unwrap();
    tokio::time::timeout(Duration::from_secs(1), observed)
        .await
        .expect("validation remains owned after response body disconnect")
        .unwrap();
}

#[path = "link_native_stop_trace_tests.rs"]
mod native_stop_trace;
