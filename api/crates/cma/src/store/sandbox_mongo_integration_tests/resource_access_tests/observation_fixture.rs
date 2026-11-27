use super::*;
use futures_util::StreamExt;
use tokio::sync::watch;

pub(super) struct Server {
    pub address: String,
    pub relay: Arc<Relay>,
    pub updates: Arc<DurableAgentUpdateRecorder>,
    pub observation: Arc<MongoAgentObservation>,
    pub reads: Arc<AgentReadService>,
    pub stall: Arc<std::sync::atomic::AtomicBool>,
    tasks: Vec<tokio::task::JoinHandle<()>>,
}
impl Drop for Server {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
    }
}

pub(super) async fn servers(f: &Fixture) -> Vec<Server> {
    servers_mode(f, true).await
}
pub(super) async fn servers_mode(f: &Fixture, enabled: bool) -> Vec<Server> {
    servers_with_capacity(f, enabled, Default::default()).await
}
pub(super) async fn servers_with_capacity(
    f: &Fixture,
    enabled: bool,
    capacity: crate::config::RelayCapacityConfig,
) -> Vec<Server> {
    start_servers(f, enabled, enabled, capacity, None).await
}

pub(super) async fn response_budget_servers(
    f: &Fixture,
    enabled: bool,
    idle_start: watch::Receiver<bool>,
) -> Vec<Server> {
    let barrier = PollBarrier::ResponseReplay(idle_start);
    start_servers(f, enabled, enabled, Default::default(), Some(barrier)).await
}

#[derive(Clone)]
enum PollBarrier {
    AgentEvents(watch::Receiver<bool>),
    ResponseReplay(watch::Receiver<bool>),
}

async fn start_servers(
    f: &Fixture,
    shared_observation: bool,
    bounded_responses: bool,
    capacity: crate::config::RelayCapacityConfig,
    idle_start: Option<PollBarrier>,
) -> Vec<Server> {
    let (auth, identity_server) = identity().await;
    let source = Arc::new(
        f.store
            .clone()
            .with_bounded_response_reads(bounded_responses),
    );
    let native = Arc::new(MongoAgentOwnership::open(&f.client, &f.name).await.unwrap());
    if shared_observation {
        // Paired tests stop their closed-path listeners before opening this
        // cohort. Remove only those disposable fixture membership records.
        f.client
            .database(&f.name)
            .collection::<Document>("cma_agent_manager_members")
            .delete_many(doc! {"_id":{"$in":["closed-observation-0","closed-observation-1"]}})
            .await
            .unwrap();
    }
    let mut servers = Vec::new();
    for index in 0..2 {
        let replica_auth = crate::auth::AuthState::in_memory(auth.fixture_mode().clone(), None);
        *replica_auth.fixture_metadata().write().await =
            auth.fixture_metadata().read().await.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            native.clone(),
            Member {
                instance_id: if shared_observation {
                    format!("observation-{index}")
                } else {
                    format!("closed-observation-{index}")
                },
                protocol: PROTOCOL_VERSION,
                relay_address: address.clone(),
                expires_at_ms: 0,
                draining: false,
                owned_agents: 0,
                max_owned_agents: 256,
            },
        )
        .await
        .unwrap();
        let store = MongoAgentObservation::open(source.clone(), coordinator.clone())
            .await
            .unwrap();
        let access = Arc::new(ResourceAccessService::new(
            Arc::new(f.store.clone().with_access_snapshot_session(true)),
            replica_auth.clone(),
        ));
        let base = f
            .reads(true)
            .with_resource_access(access.clone())
            .with_response_logs(source.response_log_reader());
        let service = AgentObservationService::new(base.clone(), store.clone());
        let reads = if shared_observation {
            base.with_observation(service.clone())
        } else {
            base
        };
        let updates = DurableAgentUpdateRecorder::new(source.clone());
        service.install_hints(updates.clone()).unwrap();
        let relay = Relay::with_capacity(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-observation-fixture"),
            capacity,
        )
        .unwrap();
        updates.install_live_source(relay.live_source());

        let commands = AgentCommandService::new(
            source.clone(),
            Arc::new(ResourceAccessService::new(
                source.clone(),
                replica_auth.clone(),
            )),
            Arc::new(fixture::Authority),
        );
        if shared_observation {
            relay
                .install_observation(store.clone(), reads.clone(), Some(commands.clone()))
                .unwrap();
        }
        let mut responses = layer1_public::responses::v2::router(
            layer1_public::responses::v2::ResponsesV2State::new(
                layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
                commands.clone(),
                reads.clone(),
            )
            .with_paged_events(bounded_responses),
        );
        let v2 = layer1_public::agent_v2::router(
            layer1_public::agent_v2::AgentV2State::new(reads.clone(), commands)
                .with_updates(updates.clone())
                .with_event_wake(false, Duration::from_secs(1)),
        );
        let agui = super::agui_budget_tests::app_with_reads(f, reads.clone(), updates.clone());
        let mut events = v2.merge(agui);
        if let Some(PollBarrier::AgentEvents(start)) = idle_start.clone() {
            // The relay must send its admission record before the ingress can
            // return public headers. Gate only the inner event body.
            events = events.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let start = start.clone();
                    async move { defer_idle_polls(next.run(request).await, start) }
                },
            ));
        }
        if let Some(PollBarrier::ResponseReplay(start)) = idle_start.clone() {
            responses = responses.layer(axum::middleware::from_fn(
                move |request: axum::extract::Request, next: axum::middleware::Next| {
                    let start = start.clone();
                    async move { defer_polls_after_replay(next.run(request).await, start) }
                },
            ));
        }
        let router = relay.authenticated_routes(events.merge(responses), replica_auth);
        let stall = Arc::new(std::sync::atomic::AtomicBool::new(false));
        let delay = stall.clone();
        let router = router.layer(axum::middleware::from_fn(
            move |request: axum::extract::Request, next: axum::middleware::Next| {
                let delay = delay.clone();
                async move {
                    if delay.load(std::sync::atomic::Ordering::SeqCst) {
                        std::future::pending::<()>().await;
                    }
                    next.run(request).await
                }
            },
        ));
        let server = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let heartbeat = tokio::spawn(async move {
            loop {
                tokio::time::sleep(Duration::from_secs(3)).await;
                coordinator.maintain().await.unwrap();
            }
        });
        servers.push(Server {
            address,
            relay,
            updates,
            observation: store,
            reads,
            stall,
            tasks: vec![server, heartbeat],
        });
    }
    servers[0].tasks.push(identity_server);
    servers
}

fn defer_idle_polls(
    response: axum::response::Response,
    start: watch::Receiver<bool>,
) -> axum::response::Response {
    use axum::http::header::CONTENT_TYPE;

    if !response.status().is_success()
        || response
            .headers()
            .get(CONTENT_TYPE)
            .is_none_or(|value| value != "text/event-stream")
    {
        return response;
    }
    // Opening the response has already validated and sought its history.
    // Polling its body also starts that observer's steady-state Mongo reads.
    let (parts, body) = response.into_parts();
    let body = futures_util::stream::unfold(
        (body.into_data_stream(), start),
        |(mut body, mut start)| async move {
            start.wait_for(|ready| *ready).await.ok()?;
            body.next().await.map(|frame| (frame, (body, start)))
        },
    );
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(body))
}

fn defer_polls_after_replay(
    response: axum::response::Response,
    start: watch::Receiver<bool>,
) -> axum::response::Response {
    if !response.status().is_success()
        || response
            .headers()
            .get(axum::http::header::CONTENT_TYPE)
            .is_none_or(|value| value != "text/event-stream")
    {
        return response;
    }
    let (parts, body) = response.into_parts();
    let body = futures_util::stream::unfold(
        (body.into_data_stream(), start, false),
        |(mut body, mut start, mut replayed)| async move {
            if replayed {
                start.wait_for(|ready| *ready).await.ok()?;
            }
            let frame = body.next().await?;
            // Axum supplies each SSE event as one body frame here, before
            // the relay or HTTP can combine frames. Let the full cold replay
            // reach the client, then hold subsequent polls until all opens finish.
            replayed |= frame.as_ref().is_ok_and(|bytes| {
                bytes
                    .split(|byte| *byte == b'\n')
                    .any(|line| line == b"id: 7")
            });
            Some((frame, (body, start, replayed)))
        },
    );
    axum::response::Response::from_parts(parts, axum::body::Body::from_stream(body))
}

#[tokio::test]
async fn response_replay_reaches_the_client_before_idle_polling_is_released() {
    use axum::body::{Body, Bytes};
    use axum::http::{header::CONTENT_TYPE, HeaderValue};
    use std::convert::Infallible;

    let frames = ["id: 6\n\n", "id: 7\n\n", "id: 8\n\n"];
    let mut response = axum::response::Response::new(Body::from_stream(
        futures_util::stream::iter(frames.map(|frame| Ok::<_, Infallible>(Bytes::from(frame)))),
    ));
    response
        .headers_mut()
        .insert(CONTENT_TYPE, HeaderValue::from_static("text/event-stream"));
    let (start, wait) = watch::channel(false);
    let mut body = defer_polls_after_replay(response, wait)
        .into_body()
        .into_data_stream();

    for frame in &frames[..2] {
        assert_eq!(body.next().await.unwrap().unwrap(), *frame);
    }
    assert!(futures_util::poll!(body.next()).is_pending());
    start.send_replace(true);
    assert_eq!(body.next().await.unwrap().unwrap(), frames[2]);
}

pub(super) async fn budget_servers(f: &Fixture) -> Vec<Server> {
    budget_servers_with_observation(f, true, None).await
}

pub(super) async fn budget_servers_with_observation(
    f: &Fixture,
    shared_observation: bool,
    idle_start: Option<watch::Receiver<bool>>,
) -> Vec<Server> {
    // Production admission deadline; exactly 100 slots detects double admission.
    start_servers(
        f,
        shared_observation,
        true,
        crate::config::RelayCapacityConfig {
            limit: 100,
            control_reserve: 0,
        },
        idle_start.map(PollBarrier::AgentEvents),
    )
    .await
}

pub(super) async fn agents(f: &mut Fixture, many: bool, distinct: bool) -> Vec<Agent> {
    let mut agents = vec![f.agent.clone()];
    if many {
        let original = f
            .store
            .resource_access_snapshot(&ResourceTarget::Agent {
                agent_id: f.agent.id.clone(),
            })
            .await
            .unwrap();
        let ids = super::super::observation::cohort(&f.store, &original).await;
        let source = f.store.agent_log(&f.agent.id, 0, 200).await.unwrap();
        for id in ids.into_iter().skip(1) {
            let mut agent = f.store.get_agent(&id).await.unwrap().unwrap();
            if distinct {
                distinct_owner(f, &original, &mut agent).await;
            }
            let documents: Vec<_> = source
                .iter()
                .cloned()
                .map(|mut entry| {
                    entry.agent_id = id.clone();
                    if let AgentLogKind::Created {
                        worktree_id,
                        workspace_id,
                    } = &mut entry.kind
                    {
                        *worktree_id = agent.worktree_id.clone();
                        *workspace_id = agent.workspace_id.clone();
                    }
                    log_document(&entry).unwrap()
                })
                .collect();
            f.store.agent_log.insert_many(documents).await.unwrap();
            agents.push(agent);
        }
    }
    for cohort in agents.chunks(16) {
        f.store = f
            .store
            .clone()
            .prepare_bounded_admission(
                &cohort
                    .iter()
                    .map(|agent| agent.id.clone())
                    .collect::<Vec<_>>(),
            )
            .await
            .unwrap();
    }
    agents
}

async fn identity() -> (Arc<crate::auth::AuthState>, tokio::task::JoinHandle<()>) {
    use axum::{
        extract::Form,
        http::HeaderMap,
        response::IntoResponse,
        routing::{get, post},
        Json, Router,
    };
    use serde_json::json;
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let base = format!("http://{}", listener.local_addr().unwrap());
    let router = Router::new()
        .route("/userinfo", get(|headers: HeaderMap| async move {
            let subject = headers.get("authorization").and_then(|value| value.to_str().ok()).and_then(|value| value.strip_prefix("Bearer observer:")).unwrap_or("invalid");
            Json(json!({"sub": subject}))
        }))
        .route("/api/v1/orgs/org/authorization", get(|headers: HeaderMap| async move {
            if headers.get("authorization").is_some_and(|value| value == "Bearer observer:shared-reader") {
                Json(json!({"id":"org", "your_role":"viewer"})).into_response()
            } else {
                axum::http::StatusCode::NOT_FOUND.into_response()
            }
        }))
        .route("/introspect", post(|Form(form): Form<std::collections::HashMap<String, String>>| async move {
            let subject = form.get("token").and_then(|token| token.strip_prefix("observer:"));
            Json(json!({"sub": subject, "active": subject.is_some(), "permissions": ["cma:sandbox:use"]}))
        }));
    let server = tokio::spawn(async move { axum::serve(listener, router).await.unwrap() });
    let auth = crate::auth::AuthState::in_memory(
        crate::auth::AuthMode::Provider {
            base_url: base.clone(),
            client_id: "observation-fixture".into(),
            client_secret: "synthetic-observation-fixture".into(),
            public_url: "https://cma.example.test".into(),
        },
        None,
    );
    *auth.fixture_metadata().write().await = Some(serde_json::from_value(json!({
        "authorization_endpoint":format!("{base}/authorize"), "token_endpoint":format!("{base}/token"),
        "userinfo_endpoint":format!("{base}/userinfo"), "introspection_endpoint":format!("{base}/introspect"),
    })).unwrap());
    (auth, server)
}

async fn distinct_owner(f: &Fixture, original: &ResourceAccessSnapshot, agent: &mut Agent) {
    let suffix = agent.id.as_str();
    let mut sandbox = original.lineage.sandbox.clone();
    sandbox.id = layer2_core::SandboxId(format!("sbx_{suffix}"));
    sandbox.owner = SubjectId::new(format!("owner_{suffix}"));
    let mut workspace = original.lineage.workspace.clone().unwrap();
    workspace.id = layer2_core::WorkspaceId(format!("wsp_{suffix}"));
    workspace.sandbox_id = sandbox.id.clone();
    workspace.owner = sandbox.owner.clone();
    let mut worktree = f
        .store
        .get_worktree(&agent.worktree_id)
        .await
        .unwrap()
        .unwrap();
    worktree.workspace_id = workspace.id.clone();
    worktree.sandbox_id = sandbox.id.clone();
    worktree.owner = sandbox.owner.clone();
    agent.workspace_id = workspace.id.clone();
    agent.sandbox_id = sandbox.id.clone();
    agent.owner = sandbox.owner.clone();
    f.store
        .sandboxes
        .insert_one(crate::store::sandboxes::sandbox_document(&sandbox, 1).unwrap())
        .await
        .unwrap();
    f.store
        .workspaces
        .insert_one(crate::store::workspaces::workspace_document(&workspace).unwrap())
        .await
        .unwrap();
    f.store
        .worktrees
        .replace_one(
            doc! {"_id": worktree.id.as_str()},
            crate::store::worktrees::worktree_document(&worktree).unwrap(),
        )
        .await
        .unwrap();
    f.store
        .agents
        .replace_one(
            doc! {"_id": agent.id.as_str()},
            crate::store::agents::agent_document(agent).unwrap(),
        )
        .await
        .unwrap();
}
