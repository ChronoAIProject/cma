//! Replay the public desktop contract, including real HTTP upgrades and binary relay.
use std::sync::Arc;

use axum::{body::Body, http::Request, response::Response, Extension};
use futures_util::{SinkExt, StreamExt};
use layer2_core::{
    profiles::EnvironmentVariable,
    sandbox::{store::SandboxStore, AgentStatus, DesktopService, SandboxStatus},
    AgentId, Authorization, Caller, Capability, SandboxId, SubjectId,
};
use tokio::{net::TcpListener, sync::Mutex};
use tokio_tungstenite::tungstenite::{Error, Message};

use super::{super::agents_tests as agents, Scenario};
use crate::AuthenticatedSubject;

mod backend;

pub(super) async fn scenario(name: &str) -> Option<Scenario> {
    if !name.starts_with("desktop_") {
        return None;
    }
    scenario_with_standing(
        name,
        Arc::new(layer2_core::account_deletion::AvailableAccounts),
    )
    .await
}

async fn scenario_with_standing(
    name: &str,
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
) -> Option<Scenario> {
    let fixture = agents::fixture().await;
    let id = SandboxId("sbx_route".into());
    let mut sandbox = fixture.store.get_sandbox(&id).await.unwrap().unwrap();
    if name != "desktop_headless" {
        sandbox
            .profile
            .content
            .environment_variables
            .push(EnvironmentVariable {
                name: "CMA_MACHINE_INCLUDED".into(),
                value: "1".into(),
            });
    }
    if name == "desktop_paused" {
        sandbox.status = SandboxStatus::Paused;
    }
    let revision = sandbox.revision;
    sandbox.revision += 1;
    fixture.store.save_sandbox(revision, sandbox).await.unwrap();
    let mut agent = agents::seeded_agent("agt_contract", AgentStatus::Ready, 2, None);
    if name == "desktop_starting" {
        agent.status = AgentStatus::Starting;
    }
    agent.profile.content.isolated_agent = name == "desktop_isolated";
    agents::insert_agent(&fixture.store, agent, vec![]).await;
    let access = Arc::new(
        layer2_core::resource_access::ResourceAccessService::new(
            fixture.store.clone(),
            Arc::new(super::super::shared_agent_tests::Authority(
                std::sync::Mutex::new(super::super::shared_agent_tests::Reply::Member),
            )),
        )
        .with_account_standing(standing),
    );
    let service = Arc::new(DesktopService::new(
        fixture
            .state
            .reads
            .with_resource_access(access)
            .with_sessions(fixture.store.clone()),
        Arc::new(layer3_adapter::codex::CodexAdapter::with_backend(
            backend::DesktopBackend {
                unavailable: name == "desktop_runtime_unavailable",
            },
            Default::default(),
        )),
    ));
    let authorization = Authorization::new([Capability::UseOwnSandboxes]);
    let held = if name == "desktop_busy" {
        Some(
            service
                .open(
                    &Caller::new(SubjectId::new("owner-a"), authorization.clone()),
                    &AgentId("agt_contract".into()),
                )
                .await
                .unwrap(),
        )
    } else {
        None
    };
    Some(Scenario {
        router: super::super::desktop_router_with_origins(
            service,
            vec![
                "https://cma.example".into(),
                "https://legacy.example".into(),
                "https://cma.nyxid.ai".into(),
                "https://bot.chrono-ai.fun".into(),
            ],
        )
        .layer(Extension(Arc::new(Mutex::new(held)))),
        subject: match name {
            "desktop_anonymous" => None,
            "desktop_other_owner" => Some("owner-b"),
            _ => Some("owner-a"),
        },
        authorization: if name == "desktop_forbidden" {
            Authorization::none()
        } else {
            authorization
        },
        symbols: Default::default(),
    })
}

/// A oneshot router cannot supply Hyper's live OnUpgrade extension. Drive the
/// same router over loopback and compare the actual handshake/refusal instead.
pub(super) async fn upgrade_response(scenario: &Scenario, request: Request<Body>) -> Response {
    let mut router = scenario.router.clone();
    if let Some(subject) = scenario.subject {
        router = router
            .layer(Extension(AuthenticatedSubject(SubjectId::new(subject))))
            .layer(Extension(scenario.authorization.clone()));
    }
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap();
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    // Abort on unwind as well as success; an assertion must not leak a server.
    let _server = AbortServer(server);
    let path = request.uri().path_and_query().unwrap().as_str();
    let mut response = if request.headers().contains_key("sec-websocket-key") {
        let mut wire = Request::builder().uri(format!("ws://{address}{path}"));
        *wire.headers_mut().unwrap() = request.headers().clone();
        wire = wire.header("host", address.to_string());
        match tokio_tungstenite::connect_async(wire.body(()).unwrap()).await {
            Ok((mut stream, response)) => {
                assert_eq!(
                    stream.next().await.unwrap().unwrap(),
                    Message::Binary(b"RFB 003.008\n".to_vec())
                );
                let input = Message::Binary(vec![5, 1, 0, 80, 0, 100]);
                stream.send(input.clone()).await.unwrap();
                assert_eq!(stream.next().await.unwrap().unwrap(), input);
                stream.close(None).await.unwrap();
                response.map(|_| Body::empty())
            }
            Err(Error::Http(response)) => response.map(|body| Body::from(body.unwrap_or_default())),
            Err(error) => panic!("desktop handshake transport failed: {error}"),
        }
    } else {
        let response = reqwest::Client::new()
            .get(format!("http://{address}{path}"))
            .headers(request.headers().clone())
            .send()
            .await
            .unwrap();
        let mut result = Response::builder().status(response.status());
        *result.headers_mut().unwrap() = response.headers().clone();
        result
            .body(Body::from(response.bytes().await.unwrap()))
            .unwrap()
    };
    // Hyper adds a transport clock header; it is not part of the route contract.
    response.headers_mut().remove("date");
    response
}

struct AbortServer(tokio::task::JoinHandle<()>);
impl Drop for AbortServer {
    fn drop(&mut self) {
        self.0.abort();
    }
}

#[tokio::test]
async fn desktop_websocket_disconnects_at_next_standing_recheck() {
    use crate::rest::shared_stream_tests::ChangingStanding;
    for deleting in ["owner-a", "unrelated", "outage"] {
        let standing = Arc::new(ChangingStanding::default());
        let scenario = scenario_with_standing("desktop_ready", standing.clone())
            .await
            .unwrap();
        let router = scenario
            .router
            .layer(Extension(AuthenticatedSubject(SubjectId::new("owner-a"))))
            .layer(Extension(scenario.authorization));
        let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = listener.local_addr().unwrap();
        let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
        let _server = AbortServer(server);
        let (mut socket, response) = tokio_tungstenite::connect_async(format!(
            "ws://{address}/api/v1/agents/agt_contract/desktop/ws"
        ))
        .await
        .unwrap();
        assert_eq!(
            response.status(),
            axum::http::StatusCode::SWITCHING_PROTOCOLS
        );
        assert_eq!(
            socket.next().await.unwrap().unwrap(),
            Message::Binary(b"RFB 003.008\n".to_vec())
        );
        *standing.0.lock().unwrap() = Some(deleting);
        tokio::time::pause();
        tokio::time::advance(std::time::Duration::from_secs(31)).await;
        if deleting == "unrelated" {
            socket.send(Message::Binary(vec![1, 2, 3])).await.unwrap();
            assert_eq!(
                socket.next().await.unwrap().unwrap(),
                Message::Binary(vec![1, 2, 3])
            );
        } else {
            assert!(matches!(
                socket.next().await.unwrap().unwrap(),
                Message::Close(_)
            ));
        }
        tokio::time::resume();
        let _ = socket.close(None).await;
    }
}
