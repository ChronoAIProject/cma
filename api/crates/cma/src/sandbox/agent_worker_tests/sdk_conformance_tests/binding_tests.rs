use super::*;
use crate::auth::{persistence::MemoryAuthStore, AuthMode, AuthState};
use layer2_core::profiles::{content_digest, AgentProvider, ProfileCapacityBounds};
use layer2_core::sandbox::binding::*;

struct BindingServer {
    base: String,
    client: reqwest::Client,
    key: ProvidedCredential,
    binding: ProviderBinding,
    agent: Agent,
    harness: Arc<Harness>,
    probe: Arc<runtime_tests::RuntimeProbe>,
    stop: tokio::sync::watch::Sender<bool>,
    worker: JoinHandle<()>,
    http: JoinHandle<()>,
}
impl Drop for BindingServer {
    fn drop(&mut self) {
        let _ = self.stop.send(true);
        self.worker.abort();
        self.http.abort();
    }
}
impl BindingServer {
    async fn new(interruptible: bool) -> Self {
        let harness = Arc::new(Harness::new(Arc::new(EmptyAgentSkillResolver)).await);
        let agent = harness.admit("binding-http", vec![]).await;
        let agent = harness.drive_ready(&agent.id).await;
        let key =
            ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
        let binding = seed(&harness, &agent, &key).await;
        let unavailable = Arc::new(MemoryAuthStore::default());
        unavailable.set_unavailable(true);
        let auth = AuthState::with_store(
            AuthMode::Trusted {
                subject: "unrelated-owner".into(),
            },
            None,
            unavailable,
        )
        .with_binding_store(harness.store.clone());
        let bindings = BindingAccessService::new(harness.store.clone());
        let probe = if interruptible {
            runtime_tests::RuntimeProbe::interruptible()
        } else {
            runtime_tests::RuntimeProbe::gated()
        };
        let (services, context) = projection_components(&harness, Some(probe.clone())).await;
        let (router, worker) = v2_tests::compose_with_authority(
            &harness,
            services,
            context,
            "binding-http-worker",
            auth.clone(),
            Some(bindings),
        );
        let router = router.layer(middleware::from_fn_with_state(
            auth,
            crate::auth::attach_subject,
        ));
        let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
        let base = format!("http://{}", listener.local_addr().unwrap());
        let http = tokio::spawn(async move {
            axum::serve(listener, router).await.unwrap();
        });
        let (stop, shutdown) = tokio::sync::watch::channel(false);
        let worker = crate::commands::CommandScheduler::new(worker).spawn(shutdown);
        Self {
            base,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(20))
                .build()
                .unwrap(),
            key,
            binding,
            agent,
            harness,
            probe,
            stop,
            worker,
            http,
        }
    }
    fn get(&self, path: &str) -> reqwest::RequestBuilder {
        self.client
            .get(format!("{}{path}", self.base))
            .bearer_auth(self.key.expose())
    }
    fn post(&self, path: &str, key: &str, body: &Value) -> reqwest::RequestBuilder {
        self.client
            .post(format!("{}{path}", self.base))
            .bearer_auth(self.key.expose())
            .header("idempotency-key", key)
            .json(body)
    }
    async fn wait_input(&self) {
        tokio::time::timeout(Duration::from_secs(10), async {
            while self.probe.inputs().is_empty() {
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap();
    }
}
async fn seed(harness: &Harness, agent: &Agent, key: &ProvidedCredential) -> ProviderBinding {
    let profile = json!({"schema_version":1,
        "metadata":{"name":"http", "owner":{"kind":"human","subject":agent.owner.as_str()},"provider":"registered"},
        "model":{"access":"managed","service":AgentProvider::default().connection_services().managed,"api_type":"responses","model":"test","thinking_effort":"medium"},
        "permission":{"action_mode":"guarded"},"context":{},"environment":{"size":{"cpu_cores":1,"memory_gib":2},"workspace":{"type":"directory","name":"project"}}});
    let digest = format!("sha256:{}", content_digest(&profile).unwrap());
    let plan = map_profile(
        profile,
        &digest,
        AgentProvider::default(),
        "responses",
        ProfileCapacityBounds {
            min_cpu_millis: 250,
            max_cpu_millis: 8000,
            min_memory_mib: 512,
            max_memory_mib: 16384,
        },
    )
    .unwrap();
    let mut binding = ProviderBinding::new(
        "binding-http".into(),
        SubjectId::new("manager"),
        plan,
        key.sha256(),
        layer2_core::now_rfc3339(),
    )
    .unwrap();
    binding.sandbox_id = agent.sandbox_id.clone();
    binding.workspace_id = agent.workspace_id.clone();
    harness
        .store
        .save_binding(None, binding.clone())
        .await
        .unwrap();
    binding.state = BindingState::Ready;
    binding.work = BindingWork::Idle;
    binding
        .advance_revision(layer2_core::now_rfc3339())
        .unwrap();
    harness
        .store
        .save_binding(Some(1), binding.clone())
        .await
        .unwrap();
    let mut conversation =
        BindingConversation::new(&binding, "external-http".into(), layer2_core::now_rfc3339())
            .unwrap();
    conversation.agent_id = agent.id.clone();
    conversation.worktree_id = agent.worktree_id.clone();
    harness
        .store
        .save_binding_conversation(binding.revision, None, conversation.clone())
        .await
        .unwrap();
    conversation.state = BindingConversationState::Ready;
    conversation.revision += 1;
    harness
        .store
        .save_binding_conversation(binding.revision, Some(1), conversation)
        .await
        .unwrap();
    binding
}

#[tokio::test]
async fn binding_http_worker_projects_tools_retains_context_and_replays_after_key_rotation() {
    let server = BindingServer::new(false).await;
    let card = server
        .client
        .get(format!("{}/.well-known/agent-card.json", server.base))
        .header("cookie", "cma_session=stale")
        .bearer_auth("stale-owner-credential")
        .send()
        .await
        .unwrap();
    assert_eq!(card.status(), StatusCode::OK);
    assert_eq!(
        card.json::<Value>().await.unwrap()["capabilities"]["activity_items"],
        true
    );
    let context = json!({"schema_version":1,"channel_id":"channel-http","conversation_id":"external-http","event_ref":"sealed-http-reference","activity":{"schema_version":1,"event_id":"event-http","full":"x".repeat(300_000)}});
    let body = json!({"conversation":"external-http","stream":true,"input":"sdk-raw-with-tools","event_context":context});
    let response = server
        .post("/api/v2/responses", "http-event", &body)
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let response_id = response.headers()["x-cma-response-id"]
        .to_str()
        .unwrap()
        .to_owned();
    server.wait_input().await;
    let inputs = server.probe.inputs();
    assert_eq!(inputs.len(), 1);
    assert!(
        matches!(&inputs[0].1, ConversationInput::RecoverablePrompt { content, .. }
        if matches!(&content[0], layer3_adapter::ContentBlock::Text(text) if text.text == "sdk-raw-with-tools\n\nevent_ref: sealed-http-reference"))
    );
    let path = "/api/v1/cmaeg/bindings/binding-http/conversations/external-http/events/event-http";
    assert_eq!(
        server
            .get(path)
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        context
    );
    server.probe.release_reply();
    let wire = response.text().await.unwrap();
    let frames: Vec<Value> = wire
        .lines()
        .filter_map(|line| line.strip_prefix("data: "))
        .map(|data| serde_json::from_str(data).unwrap())
        .collect();
    assert_eq!(frames[0]["type"], "response.created");
    assert_eq!(
        frames.last().unwrap()["type"],
        "response.completed",
        "{wire}"
    );
    assert_eq!(
        frames.last().unwrap()["response"]["conversation"]["id"],
        server.agent.id.as_str()
    );
    assert!(frames
        .iter()
        .any(|event| event["type"] == "response.mcp_call.completed"));
    assert!(wire.contains("Hello from CMA"));
    let replacement =
        ProvidedCredential::new(format!("nyx_{}", uuid::Uuid::new_v4().simple())).unwrap();
    let next = server
        .binding
        .rotate_credential(replacement.sha256(), layer2_core::now_rfc3339())
        .unwrap();
    server
        .harness
        .store
        .save_binding(Some(server.binding.revision), next)
        .await
        .unwrap();
    assert_eq!(
        server.get(path).send().await.unwrap().status(),
        StatusCode::UNAUTHORIZED
    );
    let alias = format!("/api/v2/agents/{}/responses", server.agent.id);
    let retried = server
        .client
        .post(format!("{}{alias}", server.base))
        .header("idempotency-key", "http-event")
        .json(&body)
        .bearer_auth(replacement.expose())
        .send()
        .await
        .unwrap();
    assert_eq!(retried.status(), StatusCode::OK);
    assert_eq!(retried.headers()["x-cma-response-id"], response_id);
    assert_eq!(retried.text().await.unwrap(), wire);
    assert_eq!(server.probe.inputs().len(), 1);
    assert_eq!(
        server
            .client
            .get(format!("{}{path}", server.base))
            .bearer_auth(replacement.expose())
            .send()
            .await
            .unwrap()
            .error_for_status()
            .unwrap()
            .json::<Value>()
            .await
            .unwrap(),
        context
    );
    assert_eq!(
        server
            .client
            .get(format!(
                "{}/api/v1/cmaeg/bindings/foreign/conversations/external-http/events/event-http",
                server.base
            ))
            .bearer_auth(replacement.expose())
            .send()
            .await
            .unwrap()
            .status(),
        StatusCode::FORBIDDEN
    );
}

#[tokio::test]
async fn binding_http_cancellation_targets_its_bound_response_and_records_interruption() {
    let server = BindingServer::new(true).await;
    let response = server
        .post(
            "/api/v2/responses",
            "cancel-source",
            &json!({"conversation":"external-http","stream":true,"input":"work until cancelled"}),
        )
        .send()
        .await
        .unwrap();
    assert_eq!(response.status(), StatusCode::OK);
    let id = response.headers()["x-cma-response-id"].to_str().unwrap();
    server.wait_input().await;
    let path = format!("/api/v2/agents/{}/responses/{id}/controls", server.agent.id);
    let observed: Value = server
        .get(&path)
        .send()
        .await
        .unwrap()
        .error_for_status()
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(observed["target_active"], true);
    let intent = json!({"turn_sequence":observed["target"]["turn_sequence"],"expected_agent_revision":observed["agent_revision"],"command":{"type":"cancel"}});
    let accepted = server
        .post(&path, "cancel-bound", &intent)
        .send()
        .await
        .unwrap();
    let status = accepted.status();
    let accepted: Value = accepted.json().await.unwrap();
    assert_eq!(status, StatusCode::ACCEPTED, "{accepted}");
    server.probe.release_reply();
    let wire = response.text().await.unwrap();
    assert!(wire.contains("response.incomplete"), "{wire}");
    assert!(wire.contains("interrupted"), "{wire}");
    let inputs = server.probe.inputs();
    assert_eq!(inputs.len(), 2);
    assert!(matches!(inputs[1].1, ConversationInput::Cancel));
    assert_eq!(inputs[0].0, inputs[1].0);
}
