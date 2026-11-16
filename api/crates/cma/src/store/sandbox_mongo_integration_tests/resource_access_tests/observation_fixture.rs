use super::*;

pub(super) struct Server {
    pub address: String,
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
    let (auth, identity_server) = identity().await;
    let source = Arc::new(f.store.clone().with_bounded_response_reads(enabled));
    let native = Arc::new(MongoAgentOwnership::open(&f.client, &f.name).await.unwrap());
    if enabled {
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
        let replica_auth = crate::auth::AuthState::in_memory(auth.mode.clone(), None);
        *replica_auth.metadata.write().await = auth.metadata.read().await.clone();
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let address = format!("http://{}", listener.local_addr().unwrap());
        let coordinator = OwnershipCoordinator::register(
            native.clone(),
            Member {
                instance_id: if enabled {
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
        let base = f
            .reads(true)
            .with_response_logs(source.response_log_reader());
        let service = AgentObservationService::new(base.clone(), store.clone());
        let reads = if enabled {
            base.with_observation(service.clone())
        } else {
            base
        };
        let updates = DurableAgentUpdateRecorder::new(source.clone());
        service.install_hints(updates.clone()).unwrap();
        let relay = Relay::new(
            coordinator.clone(),
            &updates,
            ring::hmac::Key::new(ring::hmac::HMAC_SHA256, b"synthetic-observation-fixture"),
        )
        .unwrap();
        updates.install_live_source(relay.live_source());
        let access = Arc::new(ResourceAccessService::new(
            source.clone(),
            replica_auth.clone(),
        ));
        let commands =
            AgentCommandService::new(source.clone(), access, Arc::new(fixture::Authority));
        if enabled {
            relay
                .install_observation(store, reads.clone(), Some(commands.clone()))
                .unwrap();
        }
        let responses = layer1_public::responses::v2::router(
            layer1_public::responses::v2::ResponsesV2State::new(
                layer1_public::agent_page::AgentPageOrigin::new("https://cma.example").unwrap(),
                commands.clone(),
                reads.clone(),
            )
            .with_paged_events(enabled),
        );
        let v2 = layer1_public::agent_v2::router(
            layer1_public::agent_v2::AgentV2State::new(reads.clone(), commands)
                .with_updates(updates.clone())
                .with_event_wake(false, Duration::from_secs(1)),
        );
        let agui = super::agui_budget_tests::app_with_reads(f, reads, updates);
        let router = relay.authenticated_routes(v2.merge(agui).merge(responses), replica_auth);
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
            tasks: vec![server, heartbeat],
        });
    }
    servers[0].tasks.push(identity_server);
    servers
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
    *auth.metadata.write().await = Some(serde_json::from_value(json!({
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
