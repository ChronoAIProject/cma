use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use futures_util::{SinkExt as _, StreamExt as _};
use layer3_adapter::wire::{AgentRef, RequestFrame, ResponseFrame, WireOperation};
use layer3_adapter::{
    AgentHomePlan, AgentLaunch, AgentSlot, CheckpointRequest, CredentialFrame, InstallRequest,
    RestoreRequest, RuntimeCredential, WorkspaceRequest, WorkspaceSource, WorktreeRequest,
};
use tokio_tungstenite::tungstenite::Message as ClientMessage;

use super::*;

fn root(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("cma-link-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&root);
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

#[tokio::test]
async fn websocket_announces_and_relays_the_opaque_conversation_lane() {
    let home = root("websocket");
    let supervisor = supervisor(home.clone());
    install_and_credential(&supervisor).await;
    let workspace = match supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest {
            name: "project".into(),
            source: WorkspaceSource::Init,
        }))
        .await
    {
        ResponseFrame::PrepareWorkspace(receipt) => receipt,
        other => panic!("workspace preparation failed: {other:?}"),
    };
    let worktree = match supervisor
        .dispatch(RequestFrame::AddWorktree(WorktreeRequest {
            workspace: "project".into(),
            name: "agent-1".into(),
            base: workspace.resolved_commit,
        }))
        .await
    {
        ResponseFrame::AddWorktree(receipt) => receipt,
        other => panic!("worktree addition failed: {other:?}"),
    };
    let materialized = supervisor
        .dispatch(RequestFrame::MaterializeAgentHome(AgentHomePlan {
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
        .dispatch(RequestFrame::OpenAgent(AgentLaunch {
            agent_id: AgentSlot::new("agent-1"),
            cwd: worktree.path,
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
    let closed = supervisor
        .dispatch(RequestFrame::CloseAgent(AgentRef {
            agent_id: AgentSlot::new("agent-1"),
        }))
        .await;
    assert!(matches!(closed, ResponseFrame::CloseAgent(_)));

    server.abort();
    let _ = server.await;
    std::fs::remove_dir_all(home).unwrap();
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
