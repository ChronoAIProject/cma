use super::*;
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame, WireFailure,
};
use layer3_adapter::{AgentLaunch, ContentBlock, ConversationChannel, RuntimeObservation};
use std::sync::atomic::AtomicBool;

struct RuntimeBoundary {
    backend: InMemoryHostedBackend,
    prompts: Arc<Mutex<Vec<Vec<ContentBlock>>>>,
    lose_prompt_response: bool,
    close_proofs: Option<Arc<AtomicUsize>>,
    restart_supported: Arc<AtomicBool>,
    close_refusal: Arc<Mutex<Option<&'static str>>>,
    replay_restart_history: bool,
    touch_sandbox_on_restart: Option<(Arc<MongoSandboxStore>, layer2_core::SandboxId)>,
}

#[async_trait::async_trait]
impl HostedBackend for RuntimeBoundary {
    async fn provision(
        &self,
        request: ProviderSandboxRequest,
    ) -> layer3_adapter::Result<RuntimeHandle> {
        self.backend.provision(request).await
    }

    async fn adopt(
        &self,
        handle: &RuntimeHandle,
        request: &ProviderSandboxRequest,
    ) -> layer3_adapter::Result<Option<RuntimeObservation>> {
        self.backend.adopt(handle, request).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        if let (Some(observations), RequestFrame::ProveAgentProcessClosed(proof)) =
            (&self.close_proofs, &request)
        {
            if let Some(code) = *self.close_refusal.lock().unwrap() {
                return Ok(ResponseFrame::Failure(WireFailure {
                    operation: request.operation(),
                    code: code.into(),
                    detail: "fixture could not verify the predecessor Close".into(),
                    retryable: false,
                }));
            }
            let response = self
                .backend
                .exchange(
                    handle,
                    RequestFrame::RuntimeStatus(proof.runtime.runtime.clone()),
                )
                .await?;
            let ResponseFrame::RuntimeStatus(status) = response else {
                panic!("expected bound runtime status");
            };
            assert!(!status.agent_process_restart);
            assert_eq!(status.agent_processes, 0);
            assert_eq!(status.boot_id, proof.runtime.boot_id);
            let requests = self.backend.requests().await;
            assert!(requests.iter().any(|(runtime, request)| runtime == handle
                && matches!(request, RequestFrame::OpenAgent(open)
                    if open.request.agent_id == proof.close.agent_id
                    && open.request.request_sha256 == proof.open_request_sha256
                    && open.request.cwd == proof.worktree_path)));
            assert!(requests.iter().any(|(runtime, request)| runtime == handle
                && matches!(request, RequestFrame::CloseAgent(close) if close.request == proof.close)));
            observations.fetch_add(1, Ordering::SeqCst);
            return Ok(ResponseFrame::ProveAgentProcessClosed(proof.clone()));
        }
        match self.backend.exchange(handle, request).await? {
            ResponseFrame::RuntimeStatus(mut status) => {
                status.agent_process_restart = self.restart_supported.load(Ordering::SeqCst);
                Ok(ResponseFrame::RuntimeStatus(status))
            }
            response => Ok(response),
        }
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        let replay_history = self.replay_restart_history && launch.restart_runtime.is_some();
        let (channel, receipt) = self
            .backend
            .open_agent(handle, launch, resume, settings, updates.clone())
            .await?;
        if replay_history {
            owner_stop::replay_retained_history(
                channel.conversation_id.as_deref().unwrap(),
                &updates,
            )
            .await?;
        }
        if let Some((store, sandbox_id)) = self
            .touch_sandbox_on_restart
            .as_ref()
            .filter(|_| replay_history)
        {
            let mut sandbox = store.get_sandbox(sandbox_id).await.unwrap().unwrap();
            let previous = sandbox.revision;
            sandbox.revision += 1;
            sandbox.last_activity_at = rfc3339_at(SystemTime::now() + Duration::from_secs(1));
            store.save_sandbox(previous, sandbox).await.unwrap();
        }
        let prompts = self.prompts.clone();
        let lose_response = self.lose_prompt_response;
        let input = ConversationInputSender::spawn(1, move |input| {
            let sender = channel.input.clone();
            let prompts = prompts.clone();
            async move {
                let task = match &input {
                    ConversationInput::Prompt(content)
                    | ConversationInput::RecoverablePrompt { content, .. } => Some(content.clone()),
                    _ => None,
                };
                sender.send(input).await?;
                if let Some(task) = task {
                    prompts.lock().unwrap().push(task);
                    if lose_response {
                        return Err(AdapterError::Unavailable(
                            "isolated runtime prompt response lost".into(),
                        ));
                    }
                }
                Ok(())
            }
        });
        Ok((
            ConversationChannel {
                input,
                conversation_id: channel.conversation_id,
            },
            receipt,
        ))
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

async fn replace_worker(
    harness: &mut Harness<MongoSandboxStore>,
    client: &mongodb::Client,
    database: &mongodb::Database,
    holder: &str,
) {
    let store = Arc::new(
        MongoSandboxStore::open(client, database.name())
            .await
            .unwrap(),
    );
    let registry = Arc::new(LiveAgentRegistry::default());
    let updates = DurableAgentUpdateRecorder::new(store.clone());
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: harness.adapter.clone(),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: updates.clone(),
            operations: AgentOperationService::new(store.clone(), registry.clone()),
        },
        AgentWorkerConfig::new(holder, DEPLOYMENT_ID),
    );
    harness.store = store;
    harness.registry = registry;
    harness.updates = updates;
}

async fn first_message(service: &Router, identity: &AgentId) -> Value {
    serde_json::from_str(
        &cross_interface_tests::read(service, &format!("/api/v1/agents/{identity}")).await,
    )
    .unwrap()
}

async fn exercise_restart(phase: &str) {
    let (client, database, mut harness) = isolated_environment().await;
    let prompts = Arc::new(Mutex::new(vec![]));
    harness.adapter = Arc::new(CodexAdapter::new(
        Arc::new(RuntimeBoundary {
            backend: harness.backend.clone(),
            prompts: prompts.clone(),
            lose_prompt_response: phase == "uncertain",
            close_proofs: None,
            restart_supported: Arc::new(AtomicBool::new(false)),
            close_refusal: Arc::new(Mutex::new(None)),
            replay_restart_history: false,
            touch_sandbox_on_restart: None,
        }),
        CodexSandboxSpec::default(),
    ));
    replace_worker(&mut harness, &client, &database, "task-before-restart").await;
    let service = router(&harness, &database, &client).await;
    let original = "  比较三个工具\n保留原文。  ";
    let body =
        json!({"name":"New task", "workspace_id":harness.workspace.id, "initial_message":original});
    let (status, receipt) = create(&service, &body, "restart-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{receipt}");
    let identity = admitted_identity(&service, &receipt).await;
    let expected = match phase {
        "queued" => "queued",
        "uncertain" => "recovery_required",
        _ => "write_confirmed",
    };
    tokio::time::timeout(Duration::from_secs(15), async {
        loop {
            let detail = first_message(&service, &identity).await;
            if (phase != "queued" || detail["status"] == "ready")
                && detail["first_message"]["state"] == expected
            {
                break;
            }
            harness
                .worker
                .run_one(&identity, SystemTime::now())
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(
        prompts.lock().unwrap().len(),
        usize::from(phase != "queued")
    );
    harness.registry.fence(&identity, None, u64::MAX);
    drop(service);
    replace_worker(&mut harness, &client, &database, "task-after-restart").await;
    let restarted = router(&harness, &database, &client).await;
    let (status, replay) = create(&restarted, &body, "restart-task").await;
    assert_eq!(status, StatusCode::ACCEPTED, "{replay}");
    assert_eq!(replay["id"], receipt["id"]);
    assert_eq!(admitted_identity(&restarted, &replay).await, identity);
    for _attempt in 0..8 {
        harness
            .worker
            .run_one(&identity, SystemTime::now())
            .await
            .unwrap();
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    let detail = first_message(&restarted, &identity).await;
    assert_eq!(detail["first_message"]["purpose"], "task");
    assert_eq!(
        detail["first_message"]["state"],
        if phase == "uncertain" {
            "recovery_required"
        } else {
            "write_confirmed"
        }
    );
    let transcript: Value = serde_json::from_str(
        &cross_interface_tests::read(&restarted, &format!("/api/v1/agents/{identity}/transcript"))
            .await,
    )
    .unwrap();
    let turns = transcript["turns"].as_array().unwrap();
    assert_eq!(turns.len(), 1, "{transcript}");
    assert_eq!(turns[0]["owner_message"], original);
    assert_eq!(prompts.lock().unwrap().len(), 1);
    database.drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_worker_restart_before_dispatch_keeps_one_task() {
    exercise_restart("queued").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_worker_restart_after_dispatch_does_not_resend() {
    exercise_restart("confirmed").await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at an isolated local replica set"]
async fn initial_task_public_uncertain_runtime_write_requires_recovery_without_resend() {
    exercise_restart("uncertain").await;
}

mod owner_stop;
