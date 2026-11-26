//! Exercise the real worker publication seam and Mongo authority after channel negotiation.
use super::*;
use crate::sandbox::agent_skills::EmptyAgentSkillResolver;
use crate::sandbox::agent_worker::{AgentWorkerConfig, AgentWorkerDependencies};
use crate::sandbox::live_agents::LiveAgentRegistry;
use crate::store::MongoSandboxStore;
use layer2_core::sandbox::store::{AgentStore, WorkspaceStore};
use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder, FirstMessageRecord};
use layer3_adapter::{ConversationInputSender, ConversationUpdate, RuntimeHandle};
use std::sync::Arc;
mod connection;
mod publication_contention;
mod runtime;

struct Fixture {
    client: mongodb::Client,
    name: String,
    store: Arc<MongoSandboxStore>,
    worker: Arc<AgentWorker>,
    agent: Agent,
    context: AgentContext,
    attachment: LiveAgentAttachment,
    evidence: RequestEvidence,
    _updates: tokio::sync::mpsc::Sender<ConversationUpdate>,
    sends: Arc<std::sync::atomic::AtomicUsize>,
}

impl Fixture {
    async fn new() -> Self {
        Self::with_app(None).await
    }
    async fn with_app(app: Option<String>) -> Self {
        Self::with_adoption(app, false).await
    }
    async fn with_adoption(app: Option<String>, enabled: bool) -> Self {
        let key = if app.is_some() {
            "CHRONO_TEST_MONGODB_FAILPOINT_URI"
        } else {
            "CHRONO_TEST_MONGODB_URI"
        };
        let uri = std::env::var(key).expect("disposable replica set");
        let mut options = mongodb::options::ClientOptions::parse(uri).await.unwrap();
        options.app_name = app;
        let client = mongodb::Client::with_options(options).unwrap();
        let name = format!("cma_open_ready_worker_{}", uuid::Uuid::new_v4().simple());
        let store = Arc::new(
            MongoSandboxStore::open(&client, &name)
                .await
                .unwrap()
                .with_segment_adoption_enabled(enabled),
        );
        let (agent, sandbox, workspace, worktree) =
            crate::store::native_prepared::tests::open_ready_fixture::hierarchy(&store).await;
        let runtime = RuntimeHandle::new(&sandbox.runtime.as_ref().unwrap().runtime_id);
        let context = AgentContext {
            agent_resource_preparation: false,
            sandbox,
            workspace,
            worktree,
            runtime,
        };
        let registry = Arc::new(LiveAgentRegistry::default());
        let updates = DurableAgentUpdateRecorder::new(store.clone());
        let (adapter, _) = layer3_adapter::codex::CodexAdapter::in_memory(Default::default());
        let worker = AgentWorker::new(
            AgentWorkerDependencies {
                store: store.clone(),
                adapter: Arc::new(adapter),
                skills: Arc::new(EmptyAgentSkillResolver),
                registry: registry.clone(),
                updates,
                operations: AgentOperationService::new(store.clone(), registry),
            },
            AgentWorkerConfig::new("open-worker", "test"),
        );
        let (sender, receiver) = tokio::sync::mpsc::channel(4);
        let sends = Arc::new(std::sync::atomic::AtomicUsize::new(0));
        let sent = sends.clone();
        let attachment = worker
            .dependencies
            .registry
            .attach(
                agent.id.clone(),
                agent.work.warm_conversation_id.clone().unwrap(),
                1,
                agent.work.lease.as_ref().unwrap().fence,
                agent.capabilities.clone(),
                ConversationInputSender::spawn(4, move |_| {
                    sent.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
                    async { Ok(()) }
                }),
                receiver,
                worker.dependencies.updates.clone(),
            )
            .unwrap();
        let debt = &agent.work.cleanup_debt[0];
        let evidence = RequestEvidence {
            step: debt.step,
            operation_id: debt.operation_id.clone(),
            request_sha256: debt.request_sha256.clone(),
        };
        Self {
            client,
            name,
            store,
            worker,
            agent,
            context,
            attachment,
            evidence,
            _updates: sender,
            sends,
        }
    }
    async fn publish(&self) -> Result<bool, SandboxStoreError> {
        let now = SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(
                layer2_core::seconds_from_rfc3339("2026-09-05T00:00:00Z").unwrap(),
            );
        self.worker
            .publish_prepared_open_ready(
                &self.agent,
                &self.context,
                &self.evidence,
                &self.attachment,
                now,
            )
            .await
    }
    async fn cleanup(self) {
        self.client.database(&self.name).drop().await.unwrap();
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_preserves_first_message_race_and_staged_channel() {
    let fixture = Fixture::new().await;
    let mut changed = fixture.agent.clone();
    let previous = changed.revision;
    changed.conversation.first_message = Some(
        FirstMessageRecord::new(
            &changed.creation_key,
            "first prompt",
            "2026-09-05T00:00:00Z",
        )
        .unwrap(),
    );
    changed.revision += 1;
    fixture
        .store
        .save_agent(previous, changed.clone())
        .await
        .unwrap();
    assert!(fixture.publish().await.unwrap());
    let ready = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(ready.status, AgentStatus::Ready);
    assert_eq!(ready.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(ready.title, changed.title);
    assert_eq!(
        ready.conversation.first_message,
        changed.conversation.first_message
    );
    let identity = fixture.attachment.identity();
    let target = LiveAgentTarget {
        agent_id: ready.id,
        conversation_id: identity.conversation_id,
        runtime_generation: identity.runtime_generation,
    };
    assert!(fixture
        .worker
        .dependencies
        .registry
        .target_is_attached(&target));
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .published_target_is_attached(&target));
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_missing_staged_channel_keeps_open_intent_recoverable() {
    let fixture = Fixture::new().await;
    fixture
        .worker
        .dependencies
        .registry
        .rollback(&fixture.attachment);
    assert!(matches!(
        fixture.publish().await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap(),
        fixture.agent
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_open_ready_parent_race_refuses_publication_and_attachment_can_roll_back() {
    let fixture = Fixture::new().await;
    let mut changed = fixture.context.workspace.clone();
    changed.revision += 1;
    fixture
        .store
        .save_workspace(fixture.context.workspace.revision, changed)
        .await
        .unwrap();
    assert!(matches!(
        fixture.publish().await,
        Err(SandboxStoreError::Conflict)
    ));
    fixture
        .worker
        .dependencies
        .registry
        .rollback(&fixture.attachment);
    let identity = fixture.attachment.identity();
    assert!(!fixture
        .worker
        .dependencies
        .registry
        .target_is_attached(&LiveAgentTarget {
            agent_id: fixture.agent.id.clone(),
            conversation_id: identity.conversation_id,
            runtime_generation: 1
        }));
    assert_eq!(
        fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap(),
        fixture.agent
    );
    fixture.cleanup().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at disposable replica set"]
async fn prepared_parent_conflict_retains_completed_open_without_waiting_for_lease_expiry() {
    let fixture = Fixture::new().await;
    // Keep preparation eligibility intact while making the observed parent
    // snapshot stale, so the atomic publication reaches its Conflict fence.
    let mut changed = fixture.context.workspace.clone();
    changed.revision += 1;
    fixture
        .store
        .save_workspace(fixture.context.workspace.revision, changed)
        .await
        .unwrap();
    assert!(matches!(
        fixture.publish().await,
        Err(SandboxStoreError::Conflict)
    ));
    assert_eq!(
        fixture
            .store
            .get_agent(&fixture.agent.id)
            .await
            .unwrap()
            .unwrap(),
        fixture.agent
    );
    let request = layer3_adapter::AgentOpenRequest::new(
        format!("agent:{}:open:v1", fixture.agent.id),
        crate::sandbox::runtime_options(
            &fixture.agent,
            &fixture.context.sandbox,
            &fixture.context.worktree,
        ),
        None,
    );
    assert_eq!(request.operation_id, fixture.evidence.operation_id);
    assert_eq!(request.request_sha256, fixture.evidence.request_sha256);
    let now = SystemTime::UNIX_EPOCH
        + std::time::Duration::from_secs(
            layer2_core::seconds_from_rfc3339("2026-09-05T00:00:00Z").unwrap(),
        );
    fixture
        .worker
        .retain_completed_open_after_conflict(
            &fixture.agent,
            &fixture.context.sandbox,
            &request,
            &fixture.attachment,
            now,
        )
        .await
        .unwrap();
    let pending = fixture
        .store
        .get_agent(&fixture.agent.id)
        .await
        .unwrap()
        .unwrap();
    assert_eq!(pending.status, AgentStatus::Starting);
    assert_eq!(pending.work.step, AgentWorkStep::PublishReady);
    assert!(pending.work.lease.is_none());
    assert!(pending.work.cleanup_debt.is_empty());
    assert_eq!(pending.work.receipts.len(), 3);
    assert_eq!(
        pending.conversation.conversation_id,
        fixture.agent.work.warm_conversation_id
    );
    let identity = fixture.attachment.identity();
    assert!(fixture
        .worker
        .dependencies
        .registry
        .staged_attachment(
            &LiveAgentTarget {
                agent_id: pending.id,
                conversation_id: identity.conversation_id,
                runtime_generation: identity.runtime_generation,
            },
            identity.authority_fence
        )
        .unwrap()
        .is_some());
    fixture.cleanup().await;
}
