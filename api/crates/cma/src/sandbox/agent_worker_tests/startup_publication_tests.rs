use super::*;

use layer2_core::sandbox::store::{AgentStopRequest, SandboxStoreError};
use layer3_adapter::hosted::{HostedAgentOpenSettings, HostedBackend};
use layer3_adapter::provider::ProviderSandboxRequest;
use layer3_adapter::wire::{
    AgentOpenReceipt, FencedReceipt, FencedRequest, RequestFrame, ResponseFrame,
};
use layer3_adapter::{AgentLaunch, ConversationChannel, RuntimeObservation};

#[derive(Clone, Copy)]
pub(super) enum Interleaving {
    ObserveOnly,
    FirstPrompt,
    FirstPromptOnReady,
    Stop,
    ParentRetire,
    Holder,
    Fence,
    Step,
    RuntimeIdentity,
    Debt,
    ParentPause,
    ParentRuntime,
    Expire,
}

pub(super) struct InterleavingBackend {
    backend: InMemoryHostedBackend,
    store: Arc<InMemorySandboxStore>,
    agent: Agent,
    operation: WireOperation,
    now: SystemTime,
    calls: AtomicUsize,
    pub(super) adoptions: AtomicUsize,
    action: Interleaving,
}

impl InterleavingBackend {
    async fn completed(&self, operation: WireOperation) {
        if operation != self.operation || self.calls.fetch_add(1, Ordering::SeqCst) != 0 {
            return;
        }
        if matches!(self.action, Interleaving::ObserveOnly) {
            return;
        }
        if matches!(self.action, Interleaving::Expire) {
            tokio::time::sleep(Duration::from_secs(30)).await;
            return;
        }
        tokio::time::sleep(Duration::from_secs(2)).await;
        let stamp = rfc3339_at(self.now + Duration::from_secs(2));
        let result = self
            .store
            .admit_agent_reusing_profile(AgentAdmission {
                binding: None,
                created_by: None,
                owner: self.agent.owner.clone(),
                workspace_id: self.agent.workspace_id.clone(),
                agent_id: AgentId::generate(),
                worktree_id: WorktreeId::generate(),
                profile: self.agent.profile.clone(),
                capabilities: self.agent.capabilities.clone(),
                title: None,
                first_message: Some(
                    FirstMessageRecord::new("trigger-race", "Inspect the workspace", &stamp)
                        .unwrap(),
                ),
                origin: AgentOrigin::Trigger {
                    trigger_id: "trigger-race".into(),
                    launch_id: "launch-race".into(),
                },
                creation_key: "trigger-race".into(),
                event_at: stamp,
            })
            .await
            .unwrap();
        assert!(matches!(result, AgentAdmissionResult::Existing(_)));
        let mut current = self.store.get_agent(&self.agent.id).await.unwrap().unwrap();
        match self.action {
            Interleaving::ObserveOnly => unreachable!(),
            Interleaving::FirstPrompt | Interleaving::FirstPromptOnReady => {
                current.title = Some("Owner's title during startup".into());
            }
            Interleaving::Stop => {
                self.store
                    .schedule_agent_stop(AgentStopRequest {
                        target: AgentControlTarget::from(&current),
                        reason: AgentStopReason::Owner,
                        requested_at: rfc3339_at(self.now + Duration::from_secs(2)),
                    })
                    .await
                    .unwrap();
                return;
            }
            Interleaving::Holder => {
                current.work.lease.as_mut().unwrap().holder = "new-worker".into()
            }
            Interleaving::Fence => {
                current.work.lease.as_mut().unwrap().fence += 1;
                current.work.next_fence += 1;
            }
            Interleaving::Step => current.work.step = AgentWorkStep::CloseProcess,
            Interleaving::RuntimeIdentity => {
                current.conversation.conversation_id = Some("new-native-identity".into());
                current.conversation.runtime_generation = Some(9);
            }
            Interleaving::Debt => current.work.cleanup_debt.clear(),
            Interleaving::ParentPause
            | Interleaving::ParentRetire
            | Interleaving::ParentRuntime => {
                let mut sandbox = self
                    .store
                    .get_sandbox(&current.sandbox_id)
                    .await
                    .unwrap()
                    .unwrap();
                let revision = sandbox.revision;
                if matches!(self.action, Interleaving::ParentPause) {
                    sandbox
                        .begin_pause(rfc3339_at(self.now + Duration::from_secs(2)), None)
                        .unwrap();
                } else if matches!(self.action, Interleaving::ParentRetire) {
                    sandbox
                        .begin_retirement(rfc3339_at(self.now + Duration::from_secs(2)))
                        .unwrap();
                } else {
                    sandbox.runtime.as_mut().unwrap().runtime_id = "replacement-runtime".into();
                    sandbox.revision += 1;
                }
                self.store.save_sandbox(revision, sandbox).await.unwrap();
                return;
            }
            Interleaving::Expire => unreachable!(),
        }
        let revision = current.revision;
        current.revision += 1;
        self.store.save_agent(revision, current).await.unwrap();
    }
}

#[async_trait::async_trait]
impl HostedBackend for InterleavingBackend {
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
        self.adoptions.fetch_add(1, Ordering::SeqCst);
        self.backend.adopt(handle, request).await
    }

    async fn exchange(
        &self,
        handle: &RuntimeHandle,
        request: RequestFrame,
    ) -> layer3_adapter::Result<ResponseFrame> {
        let operation = request.operation();
        let response = self.backend.exchange(handle, request).await?;
        self.completed(operation).await;
        Ok(response)
    }

    async fn open_agent(
        &self,
        handle: &RuntimeHandle,
        launch: FencedRequest<AgentLaunch>,
        resume_conversation: Option<String>,
        settings: HostedAgentOpenSettings,
        updates: mpsc::Sender<ConversationUpdate>,
    ) -> layer3_adapter::Result<(ConversationChannel, FencedReceipt<AgentOpenReceipt>)> {
        let interleave = resume_conversation.is_some()
            == matches!(self.action, Interleaving::FirstPromptOnReady);
        let response = self
            .backend
            .open_agent(handle, launch, resume_conversation, settings, updates)
            .await?;
        if interleave {
            self.completed(WireOperation::OpenAgent).await;
        }
        Ok(response)
    }

    async fn teardown(&self, handle: &RuntimeHandle) -> layer3_adapter::Result<()> {
        self.backend.teardown(handle).await
    }
}

pub(super) fn interleave(
    harness: &mut Harness,
    agent: &Agent,
    operation: WireOperation,
    action: Interleaving,
) -> Arc<InterleavingBackend> {
    let backend = Arc::new(InterleavingBackend {
        backend: harness.backend.clone(),
        store: harness.store.clone(),
        agent: agent.clone(),
        operation,
        now: harness.now,
        calls: AtomicUsize::new(0),
        adoptions: AtomicUsize::new(0),
        action,
    });
    let adapter = Arc::new(CodexAdapter::new(
        backend.clone(),
        CodexSandboxSpec::default(),
    ));
    let mut config = AgentWorkerConfig::new("agent-worker", DEPLOYMENT_ID);
    config.lease_duration = Duration::from_secs(30);
    config.open_timeout = Duration::from_secs(10);
    harness.worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: harness.store.clone(),
            adapter,
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: harness.registry.clone(),
            updates: harness.updates.clone(),
            operations: AgentOperationService::new(harness.store.clone(), harness.registry.clone()),
        },
        config,
    );
    backend
}

#[tokio::test(start_paused = true)]
async fn first_prompt_attached_during_add_worktree_advances_without_replaying_effect() {
    completed_step_with_first_prompt(
        WireOperation::AddWorktree,
        0,
        AgentWorkStep::MaterializeHome,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn first_prompt_attached_during_materialize_home_advances_without_replaying_effect() {
    completed_step_with_first_prompt(
        WireOperation::MaterializeAgentHome,
        1,
        AgentWorkStep::OpenConversation,
    )
    .await;
}

#[tokio::test(start_paused = true)]
async fn first_prompt_attached_during_open_preserves_one_native_identity_and_prompt() {
    completed_step_with_first_prompt(WireOperation::OpenAgent, 2, AgentWorkStep::PublishReady)
        .await;
}

#[tokio::test(start_paused = true)]
async fn first_prompt_attached_during_ready_reattachment_preserves_one_native_identity_and_prompt()
{
    completed_step_with_first_prompt(
        WireOperation::OpenAgent,
        3,
        AgentWorkStep::PublishConnection,
    )
    .await;
}

async fn completed_step_with_first_prompt(
    operation: WireOperation,
    prior_steps: usize,
    next: AgentWorkStep,
) {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    harness.now = SystemTime::now();
    let agent = harness.admit("startup-race", vec![]).await;
    for _ in 0..prior_steps {
        harness.run(&agent.id, harness.now).await;
    }
    if prior_steps == 3 {
        harness.registry = Arc::new(LiveAgentRegistry::default());
    }
    let action = if prior_steps == 3 {
        Interleaving::FirstPromptOnReady
    } else {
        Interleaving::FirstPrompt
    };
    let backend = interleave(&mut harness, &agent, operation, action);
    harness.run(&agent.id, harness.now).await;
    let completed = harness.stored(&agent.id).await;
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    assert_eq!(completed.work.step, next);
    assert!(completed.work.lease.is_none());
    assert!(completed.work.cleanup_debt.is_empty());
    assert_eq!(completed.work.receipts.len(), (prior_steps + 1).min(3));
    if prior_steps < 3 {
        assert_eq!(
            completed.work.receipts.last().unwrap().completed_at,
            rfc3339_at(harness.now + Duration::from_secs(2))
        );
    }
    assert_eq!(
        completed.conversation.first_message.as_ref().unwrap().text,
        "Inspect the workspace"
    );
    assert_eq!(
        completed.title.as_deref(),
        Some("Owner's title during startup")
    );
    let native_id = completed.conversation.conversation_id.clone();
    for _ in 0..5 - prior_steps {
        harness
            .run(&agent.id, harness.now + Duration::from_secs(2))
            .await;
    }
    let dispatched = harness.stored(&agent.id).await;
    let first = dispatched.conversation.first_message.as_ref().unwrap();
    assert_eq!(dispatched.status, AgentStatus::Working);
    assert_eq!(first.state, FirstMessageDelivery::WriteConfirmed);
    assert_eq!(first.attempt, 1);
    if native_id.is_some() {
        assert_eq!(dispatched.conversation.conversation_id, native_id);
    }
    assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
    let snapshot = harness.backend.snapshot(&harness.runtime).await.unwrap();
    assert_eq!(snapshot.worktrees.len(), 1);
    assert_eq!(snapshot.homes.len(), 1);
    assert_eq!(snapshot.agents.len(), 1);
    let logs = harness.store.agent_log(&agent.id, 0, 40).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::OwnerMessage { .. }))
            .count(),
        1
    );
    for (index, entry) in logs.iter().enumerate() {
        assert_eq!(entry.seq, index as u64 + 1);
    }
}

#[tokio::test(start_paused = true)]
async fn completed_worktree_does_not_publish_over_changed_authority_or_parent() {
    for action in [
        Interleaving::Stop,
        Interleaving::ParentRetire,
        Interleaving::Holder,
        Interleaving::Fence,
        Interleaving::Step,
        Interleaving::RuntimeIdentity,
        Interleaving::Debt,
        Interleaving::ParentPause,
        Interleaving::ParentRuntime,
        Interleaving::Expire,
    ] {
        let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let agent = harness.admit("startup-authority", vec![]).await;
        let backend = interleave(&mut harness, &agent, WireOperation::AddWorktree, action);
        assert!(matches!(
            harness.worker.run_one(&agent.id, harness.now).await,
            Err(SandboxStoreError::Conflict)
        ));
        let current = harness.stored(&agent.id).await;
        assert!(current.work.receipts.is_empty());
        assert_eq!(backend.calls.load(Ordering::SeqCst), 1);
        assert_eq!(
            harness
                .backend
                .snapshot(&harness.runtime)
                .await
                .unwrap()
                .worktrees
                .len(),
            1
        );
        if !matches!(action, Interleaving::Stop) {
            assert!(current.work.lease.is_some());
        }
    }
}

#[tokio::test(start_paused = true)]
async fn first_prompt_attached_before_sign_in_notice_preserves_both_logs() {
    let mut harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let mut agent = super::sign_in_tests::opening_agent(&harness, "notice-race").await;
    let parent = harness
        .store
        .get_sandbox(&agent.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    let backend = interleave(
        &mut harness,
        &agent,
        WireOperation::OpenAgent,
        Interleaving::FirstPrompt,
    );
    let (sender, receiver) = mpsc::channel(4);
    let opening = async {
        backend.completed(WireOperation::OpenAgent).await;
        sender
            .send(ConversationUpdate::SignInRequired {
                url: "https://login.example.test/device".into(),
                code: None,
                message: None,
            })
            .await
            .unwrap();
        tokio::task::yield_now().await;
        Ok(ConversationChannel {
            input: ConversationInputSender::spawn(4, |_| async { Ok(()) }),
            conversation_id: Some("notice-native".into()),
        })
    };
    harness
        .worker
        .await_startup_open(
            &mut agent,
            &parent,
            receiver,
            opening,
            Duration::from_secs(10),
            harness.now,
        )
        .await
        .unwrap();
    let current = harness.stored(&agent.id).await;
    assert_eq!(current, agent);
    assert_eq!(
        current.conversation.first_message.as_ref().unwrap().text,
        "Inspect the workspace"
    );
    assert_eq!(
        current.title.as_deref(),
        Some("Owner's title during startup")
    );
    assert!(current.conversation.conversation_id.is_none());
    assert_eq!(current.work.step, AgentWorkStep::OpenConversation);
    assert_eq!(current.work.cleanup_debt.len(), 1);
    let logs = harness.store.agent_log(&agent.id, 0, 40).await.unwrap();
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::FirstMessageChanged { .. }))
            .count(),
        1
    );
    assert_eq!(
        logs.iter()
            .filter(|entry| matches!(entry.kind, AgentLogKind::SignInRequired { .. }))
            .count(),
        1
    );
}

async fn claimed_startup(harness: &Harness) -> (Agent, Sandbox) {
    let admitted = harness.admit("publication-fence", vec![]).await;
    let current = harness
        .store
        .claim_agent_work(
            &admitted.id,
            "agent-worker",
            rfc3339_at(harness.now),
            rfc3339_at(harness.now + Duration::from_secs(30)),
        )
        .await
        .unwrap()
        .unwrap();
    let parent = harness
        .store
        .get_sandbox(&current.sandbox_id)
        .await
        .unwrap()
        .unwrap();
    (current, parent)
}

#[tokio::test(start_paused = true)]
async fn startup_publication_checks_expiry_before_first_attempt_and_after_conflict() {
    for expires_during_conflict in [false, true] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let (current, parent) = claimed_startup(&harness).await;
        let calls = AtomicUsize::new(0);
        let now = if expires_during_conflict {
            harness.now
        } else {
            harness.now + Duration::from_secs(30)
        };
        let result = harness
            .worker
            .retry_startup_publication(current.clone(), &parent, now, |mut candidate, _| {
                calls.fetch_add(1, Ordering::SeqCst);
                let store = harness.store.clone();
                async move {
                    let revision = candidate.revision;
                    candidate.revision += 1;
                    candidate.title = Some("Owner title".into());
                    store.save_agent(revision, candidate).await.unwrap();
                    tokio::time::sleep(Duration::from_secs(30)).await;
                    Err::<(), _>(SandboxStoreError::Conflict)
                }
            })
            .await;
        assert!(matches!(result, Err(SandboxStoreError::Conflict)));
        assert_eq!(
            calls.load(Ordering::SeqCst),
            usize::from(expires_during_conflict)
        );
        let after = harness.stored(&current.id).await;
        assert_eq!(after.work, current.work);
        assert!(after.work.receipts.is_empty());
    }
}

#[tokio::test]
async fn startup_publication_bounds_metadata_retries_without_releasing_lease() {
    let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
    let (current, parent) = claimed_startup(&harness).await;
    let calls = AtomicUsize::new(0);
    let result = harness
        .worker
        .retry_startup_publication(current.clone(), &parent, harness.now, |mut candidate, _| {
            let attempt = calls.fetch_add(1, Ordering::SeqCst);
            let store = harness.store.clone();
            async move {
                let revision = candidate.revision;
                candidate.revision += 1;
                candidate.title = Some(format!("Owner title {attempt}"));
                store.save_agent(revision, candidate).await.unwrap();
                Err::<(), _>(SandboxStoreError::Conflict)
            }
        })
        .await;
    assert!(matches!(result, Err(SandboxStoreError::Conflict)));
    assert_eq!(calls.load(Ordering::SeqCst), 8);
    assert_eq!(harness.stored(&current.id).await.work, current.work);
}

#[tokio::test]
async fn startup_publication_does_not_retry_ambiguous_or_nonrevision_failures() {
    for error in [
        SandboxStoreError::Unavailable("unknown commit".into()),
        SandboxStoreError::SequenceConflict,
        SandboxStoreError::NotFound,
        SandboxStoreError::Corrupt("invalid log".into()),
    ] {
        let harness = Harness::new(Arc::new(EmptyAgentSkillResolver)).await;
        let (current, parent) = claimed_startup(&harness).await;
        let expected = error.to_string();
        let mut error = Some(error);
        let result = harness
            .worker
            .retry_startup_publication(current.clone(), &parent, harness.now, |_, _| {
                std::future::ready(Err::<(), _>(
                    error
                        .take()
                        .expect("must not repeat an uncertain publication"),
                ))
            })
            .await;
        assert_eq!(result.unwrap_err().to_string(), expected);
        assert_eq!(harness.stored(&current.id).await, current);
    }
}
