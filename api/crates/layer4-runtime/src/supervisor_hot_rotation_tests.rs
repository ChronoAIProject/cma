use std::convert::Infallible;
use std::sync::Arc;
use std::time::Duration;

use axum::body::{Body, Bytes};
use axum::http::{header, HeaderMap};
use axum::routing::post;
use axum::Router;
use layer3_adapter::wire::{FencedRequest, RequestFrame, ResponseFrame};
use layer3_adapter::{
    AgentCloseRequest, AgentHomePlan, AgentLaunch, AgentSlot, CredentialFrame, InstallRequest,
    RuntimeCredential, WorkspaceRequest, WorkspaceSource, WorktreeRequest,
};
use tokio::sync::{mpsc, Mutex};

use super::{Phase, RuntimeConfig, Supervisor, SupervisorBoundary};
use crate::bridge::{Bridge, Outbound};

struct FixtureRoot(std::path::PathBuf);

impl Drop for FixtureRoot {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Server(tokio::task::JoinHandle<()>);

impl Drop for Server {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn rotate(supervisor: &Supervisor, generation: u64, value: &str) -> ResponseFrame {
    tokio::time::timeout(
        Duration::from_secs(2),
        supervisor.dispatch(RequestFrame::Credential(CredentialFrame {
            operation_id: format!("hot-rotation-{generation}"),
            value: RuntimeCredential::new(value),
            custody_generation: Some(generation),
            github_route: None,
        })),
    )
    .await
    .expect("credential delivery must not wait for an upstream stream to finish")
}

async fn echo(
    bridge: &Bridge,
    output: &mut tokio::sync::broadcast::Receiver<Outbound>,
    text: &[u8],
) {
    assert!(bridge.send(text.to_vec()).await);
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(2), output.recv())
            .await
            .expect("the same Agent conversation still responds")
            .unwrap(),
        Outbound::Frame(text.to_vec())
    );
}

async fn open_echo(supervisor: &Supervisor, config: &RuntimeConfig) -> Arc<Bridge> {
    let ResponseFrame::PrepareWorkspace(workspace) = supervisor
        .dispatch(RequestFrame::PrepareWorkspace(WorkspaceRequest::new(
            "workspace:wks_rotation:prepare:v1",
            "wks_rotation",
            "project",
            WorkspaceSource::Init,
        )))
        .await
    else {
        panic!("fixture Workspace preparation failed")
    };
    let ResponseFrame::AddWorktree(worktree) = supervisor
        .dispatch(RequestFrame::AddWorktree(FencedRequest::new(
            1,
            WorktreeRequest::new(
                "worktree:project:agent-1:add:v1",
                "project",
                "agent-1",
                workspace.resolved_commit,
            ),
        )))
        .await
    else {
        panic!("fixture Worktree preparation failed")
    };
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::MaterializeAgentHome(FencedRequest::new(
                1,
                AgentHomePlan {
                    operation_id: "agent:agent-1:home:create:v1".into(),
                    request_sha256: "a".repeat(64),
                    agent_id: AgentSlot::new("agent-1"),
                    files: Vec::new(),
                    symlinks: Vec::new(),
                    skills: Vec::new(),
                },
            )))
            .await,
        ResponseFrame::MaterializeAgentHome(_)
    ));
    let opened = supervisor
        .dispatch(RequestFrame::OpenAgent(FencedRequest::new(
            1,
            AgentLaunch {
                operation_id: "agent:agent-1:open:v1".into(),
                request_sha256: "b".repeat(64),
                agent_id: AgentSlot::new("agent-1"),
                cwd: worktree.receipt.path,
                home: config.agent_home("agent-1").to_string_lossy().into_owned(),
                program: "/bin/sh".into(),
                args: vec![
                    "-c".into(),
                    "while IFS= read -r line; do printf '%s\\n' \"$line\"; done".into(),
                ],
                env: Default::default(),
            },
        )))
        .await;
    assert!(matches!(opened, ResponseFrame::OpenAgent(_)), "{opened:?}");
    supervisor.bridge("agent-1").await.unwrap()
}

#[tokio::test]
async fn hot_credential_rotation_preserves_open_stream_process_and_conversation() {
    let root = FixtureRoot(
        std::env::temp_dir().join(format!("cma-hot-rotation-{}", uuid::Uuid::new_v4())),
    );
    let upstream = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let upstream_url = format!("http://{}", upstream.local_addr().unwrap());
    let (captured, mut observed) = mpsc::channel(8);
    let (release, chunks) = mpsc::channel::<Result<Bytes, Infallible>>(2);
    let chunks = Arc::new(Mutex::new(Some(chunks)));
    let app = Router::new().route(
        "/responses",
        post(move |headers: HeaderMap, body: String| {
            let captured = captured.clone();
            let chunks = chunks.clone();
            async move {
                captured.send(headers).await.unwrap();
                if body == "held" {
                    let receiver = chunks.lock().await.take().unwrap();
                    Body::from_stream(futures_util::stream::unfold(
                        receiver,
                        |mut receiver| async { receiver.recv().await.map(|item| (item, receiver)) },
                    ))
                } else {
                    Body::from("subsequent request completed")
                }
            }
        }),
    );
    let _upstream_server = Server(tokio::spawn(async move {
        axum::serve(upstream, app).await.unwrap();
    }));
    let reserved = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
    let proxy_bind = reserved.local_addr().unwrap();
    let mut config = RuntimeConfig::for_test(root.0.join("home"));
    config.model_proxy = Some(crate::config::ProxyConfig {
        bind: proxy_bind,
        upstream: upstream_url,
        role: "model",
    });
    drop(reserved);
    let supervisor = Supervisor::new(config.clone(), SupervisorBoundary::default()).unwrap();
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::Install(InstallRequest::new(
                "rotation-install",
                Vec::new()
            )))
            .await,
        ResponseFrame::Install(_)
    ));
    let key = |generation| format!("rotation-fixture-value-{generation}");
    assert!(matches!(
        rotate(&supervisor, 1, &key(1)).await,
        ResponseFrame::Credential(_)
    ));
    let bridge = open_echo(&supervisor, &config).await;
    let pid = bridge
        .writer_session_root()
        .expect("live Agent process has an identity");
    let boot_id = supervisor.boot_id.clone();
    let stream_id = bridge.retained_output.stream_id();
    let mut output = bridge.subscribe();
    echo(&bridge, &mut output, b"before rotation").await;

    let client = reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .build()
        .unwrap();
    let endpoint = format!("http://{proxy_bind}/responses");
    let prefix = vec![b'a'; 4096];
    release.send(Ok(Bytes::from(prefix.clone()))).await.unwrap();
    let mut held = client.post(&endpoint).body("held").send().await.unwrap();
    assert_eq!(held.status(), axum::http::StatusCode::OK);
    assert_eq!(
        observed.recv().await.unwrap()[header::AUTHORIZATION],
        format!("Bearer {}", key(1))
    );
    let mut received = tokio::time::timeout(Duration::from_secs(2), held.chunk())
        .await
        .unwrap()
        .unwrap()
        .expect("the response is already streaming")
        .to_vec();
    assert!(!received.is_empty());
    assert!(matches!(
        rotate(&supervisor, 2, &key(2)).await,
        ResponseFrame::Credential(_)
    ));
    // This stream is still open: rotation has neither drained it nor synthesized EOF.
    assert!(
        tokio::time::timeout(Duration::from_millis(25), held.chunk())
            .await
            .is_err()
    );

    for (generation, value, code) in [
        (1, key(1), "credential_generation_rollback"),
        (2, key(3), "credential_generation_conflict"),
    ] {
        let ResponseFrame::Failure(failure) = rotate(&supervisor, generation, &value).await else {
            panic!("stale or conflicting replacement must be refused")
        };
        assert_eq!(failure.code, code);
    }
    let next = client.post(&endpoint).body("next").send().await.unwrap();
    assert_eq!(next.text().await.unwrap(), "subsequent request completed");
    assert_eq!(
        observed.recv().await.unwrap()[header::AUTHORIZATION],
        format!("Bearer {}", key(2))
    );
    assert!(
        observed.try_recv().is_err(),
        "rotation must not repeat an upstream request"
    );
    let suffix = b"held response completed";
    release.send(Ok(Bytes::from_static(suffix))).await.unwrap();
    drop(release);
    while let Some(chunk) = tokio::time::timeout(Duration::from_secs(2), held.chunk())
        .await
        .unwrap()
        .unwrap()
    {
        received.extend_from_slice(&chunk);
    }
    assert_eq!(received, [prefix.as_slice(), suffix].concat());

    let retained = supervisor.bridge("agent-1").await.unwrap();
    assert!(Arc::ptr_eq(&bridge, &retained));
    assert_eq!(retained.writer_session_root(), Some(pid));
    assert!(!retained.is_ended());
    assert_eq!(supervisor.boot_id, boot_id);
    assert_eq!(supervisor.phase().await, Phase::Credentialed);
    assert_eq!(retained.retained_output.stream_id(), stream_id);
    echo(&retained, &mut output, b"after rotation").await;
    let history = retained.retained_output.snapshot(0).await.unwrap();
    assert_eq!(history.frames.len(), 2);
    assert_eq!(history.frames[0].bytes.as_ref(), b"before rotation");
    assert_eq!(history.frames[1].bytes.as_ref(), b"after rotation");
    assert_eq!(supervisor.agents.read().await.len(), 1);
    assert!(matches!(
        supervisor
            .dispatch(RequestFrame::CloseAgent(FencedRequest::new(
                1,
                AgentCloseRequest::new("agent:agent-1:close:v1", AgentSlot::new("agent-1")),
            )))
            .await,
        ResponseFrame::CloseAgent(_)
    ));
}
