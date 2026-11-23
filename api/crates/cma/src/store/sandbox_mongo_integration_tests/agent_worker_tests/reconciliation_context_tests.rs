use super::*;
use std::collections::BTreeMap;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::SystemTime;

use layer2_core::sandbox::{AgentOperationService, DurableAgentUpdateRecorder};
use layer2_core::{RuntimeCapabilities, RuntimeCapability};
use layer3_adapter::codex::{CodexAdapter, CodexSandboxSpec};
use layer3_adapter::hosted::InMemoryHostedBackend;
use layer3_adapter::ConversationInputSender;

use crate::sandbox::binding_lifecycle_test_support::{
    AgentWorker, AgentWorkerConfig, AgentWorkerDependencies, EmptyAgentSkillResolver,
};
use crate::sandbox::LiveAgentRegistry;

mod cpu_profile;
mod eligibility_tests;
#[path = "failure_pacing_tests/mod.rs"]
mod failure_pacing_tests;
mod ownership_skip_tests;
mod transport_migration_tests;
mod validation_reuse_tests;

// These attached streams emit no updates. Keep their asynchronous checkpoint
// restoration separate from the reconciliation reads measured by this test.
struct IdleRecorder;

struct LocalReadiness(Arc<AtomicBool>);

#[async_trait::async_trait]
impl layer3_adapter::ConversationSettingsSupport for LocalReadiness {
    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn managed_context_may_be_ready(&self) -> bool {
        self.0.load(Ordering::Acquire)
    }
}

#[async_trait::async_trait]
impl layer2_core::sandbox::AgentUpdateRecorder for IdleRecorder {
    async fn record_update(
        &self,
        _id: &AgentId,
        _generation: u64,
        _assembly: &mut layer2_core::sandbox::AgentMessageAssembly,
        _update: layer3_adapter::ConversationUpdate,
        _at: String,
    ) -> Result<
        layer2_core::sandbox::AgentUpdateOutcome,
        layer2_core::sandbox::AgentConversationError,
    > {
        panic!("idle fixture must not emit runtime updates")
    }
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn context_discovery_defers_ineligible_reads_but_preserves_refresh_and_recovery() {
    Box::pin(context_discovery_contract(false)).await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn validation_reuse_preserves_context_refresh_and_runtime_readiness() {
    Box::pin(context_discovery_contract(true)).await;
}

async fn context_discovery_contract(reuse: bool) {
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let name = format!("cma_reconcile_context_{}", uuid::Uuid::new_v4().simple());
    let reads = Arc::new(Mutex::new(BTreeMap::<String, usize>::new()));
    let captured = reads.clone();
    let observed_database = name.clone();
    let mut options = ClientOptions::parse(uri).await.unwrap();
    // Command payload observation is confined to this tiny disposable fixture.
    // Production retains only numeric connection-pool monitoring.
    options.command_event_handler = Some(EventHandler::callback(move |event| {
        if let CommandEvent::Started(event) = event {
            if event.db == observed_database && event.command.get_str("find").ok() == Some(AGENTS) {
                if let Ok(id) = event
                    .command
                    .get_document("filter")
                    .and_then(|filter| filter.get_str("_id"))
                {
                    *captured.lock().unwrap().entry(id.into()).or_default() += 1;
                }
            }
        }
    }));
    let client = Client::with_options(options).unwrap();
    let store = Arc::new(
        MongoSandboxStore::open(&client, &name)
            .await
            .unwrap()
            .with_agent_connection_observations(reuse),
    );
    let mut applied = idle_connection_fixture(&store, "applied_context", AgentStatus::Ready).await;
    applied.conversation.managed_guidance_revision = 2;
    applied.revision += 1;
    replace_agent_fixture(&store, &applied).await;
    let mut pending = idle_connection_fixture(&store, "pending_context", AgentStatus::Ready).await;
    let mut settings = pending.settings();
    settings.model = "model-a".into();
    pending.conversation.settings = Some(settings.clone());
    pending.revision += 1;
    replace_agent_fixture(&store, &pending).await;
    let unconfigured =
        idle_connection_fixture(&store, "unconfigured_context", AgentStatus::Ready).await;
    let mut busy = idle_connection_fixture(&store, "busy_context", AgentStatus::Working).await;
    busy.conversation.settings = Some(settings.clone());
    busy.revision += 1;
    replace_agent_fixture(&store, &busy).await;
    let mut detached =
        idle_connection_fixture(&store, "detached_context", AgentStatus::Ready).await;
    detached.conversation.managed_guidance_revision = 2;
    detached.revision += 1;
    replace_agent_fixture(&store, &detached).await;
    let detached_busy =
        idle_connection_fixture(&store, "detached_busy_context", AgentStatus::Working).await;
    let mut local_blocked =
        idle_connection_fixture(&store, "local_blocked_context", AgentStatus::Ready).await;
    local_blocked.conversation.settings = Some(settings.clone());
    local_blocked.revision += 1;
    replace_agent_fixture(&store, &local_blocked).await;
    let mut unsupported =
        idle_connection_fixture(&store, "unsupported_context", AgentStatus::Ready).await;
    unsupported.conversation.settings = Some(settings.clone());
    unsupported.revision += 1;
    replace_agent_fixture(&store, &unsupported).await;

    let registry = Arc::new(LiveAgentRegistry::default());
    let updates = DurableAgentUpdateRecorder::new(store.clone());
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: updates.clone(),
            operations: AgentOperationService::new(store.clone(), registry.clone()),
        },
        AgentWorkerConfig::new("reconcile-test", "reconcile-test").with_managed_documents(true),
    );
    let mut senders = Vec::new();
    let local_ready = Arc::new(AtomicBool::new(false));
    for agent in [
        &applied,
        &pending,
        &unconfigured,
        &busy,
        &local_blocked,
        &unsupported,
    ] {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        senders.push(sender);
        let mut input = ConversationInputSender::spawn(1, |_| async { Ok(()) });
        if agent.id != unsupported.id {
            let ready = if agent.id == local_blocked.id {
                local_ready.clone()
            } else {
                Arc::new(AtomicBool::new(true))
            };
            input = input.with_settings_support(Arc::new(LocalReadiness(ready)));
        }
        let attachment = registry
            .attach(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                agent.conversation.runtime_generation.unwrap(),
                agent.work.next_fence - 1,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                input,
                receiver,
                Arc::new(IdleRecorder),
            )
            .unwrap();
        registry.publish_and_activate(&attachment).unwrap();
    }

    reads.lock().unwrap().clear();
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        2
    );
    let observed = reads.lock().unwrap().clone();
    assert_eq!(observed.get(applied.id.as_str()), None);
    assert_eq!(observed.get(unconfigured.id.as_str()), None);
    assert_eq!(observed.get(busy.id.as_str()), None);
    assert_eq!(observed.get(local_blocked.id.as_str()), None);
    assert_eq!(observed.get(unsupported.id.as_str()), None);
    assert!(
        observed
            .get(pending.id.as_str())
            .copied()
            .unwrap_or_default()
            > 0
    );
    assert!(
        observed
            .get(detached.id.as_str())
            .copied()
            .unwrap_or_default()
            > 0
    );
    assert_eq!(
        store.get_agent(&applied.id).await.unwrap(),
        Some(applied.clone())
    );
    let recovered = store.get_agent(&detached.id).await.unwrap().unwrap();
    assert_eq!(recovered.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(recovered.conversation, detached.conversation);
    let recovered_busy = store.get_agent(&detached_busy.id).await.unwrap().unwrap();
    assert_eq!(recovered_busy.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(recovered_busy.conversation, detached_busy.conversation);

    // Deferral is not cached: an owner model selection or settled turn becomes
    // eligible on the very next sweep and goes through the ordinary fresh read.
    let mut configured = unconfigured.clone();
    configured.conversation.settings = Some(settings);
    configured.revision += 1;
    replace_agent_fixture(&store, &configured).await;
    let mut settled = busy.clone();
    settled.status = AgentStatus::Ready;
    settled.conversation.active_turn = None;
    settled.revision += 1;
    replace_agent_fixture(&store, &settled).await;
    local_ready.store(true, Ordering::Release);
    reads.lock().unwrap().clear();
    worker
        .reconcile_connections(SystemTime::now())
        .await
        .unwrap();
    let observed = reads.lock().unwrap().clone();
    for agent in [&configured, &settled, &local_blocked] {
        assert!(observed.get(agent.id.as_str()).copied().unwrap_or_default() > 0);
    }
    assert_eq!(observed.get(unsupported.id.as_str()), None);
    // A positive local hint cannot admit maintenance without the actual receipt.
    assert_eq!(
        store.get_agent(&local_blocked.id).await.unwrap(),
        Some(local_blocked.clone())
    );
    assert_eq!(
        store.get_agent(&configured.id).await.unwrap(),
        Some(configured)
    );
    assert_eq!(store.get_agent(&settled.id).await.unwrap(), Some(settled));

    // Neither direction is cached, and a locally deferred attachment still
    // recovers through the ordinary durable path after it disappears.
    local_ready.store(false, Ordering::Release);
    reads.lock().unwrap().clear();
    worker
        .reconcile_connections(SystemTime::now())
        .await
        .unwrap();
    assert_eq!(reads.lock().unwrap().get(local_blocked.id.as_str()), None);
    registry.fence(
        &local_blocked.id,
        local_blocked.conversation.runtime_generation,
        local_blocked.work.next_fence - 1,
    );
    assert_eq!(
        worker
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        1
    );
    let recovered_local = store.get_agent(&local_blocked.id).await.unwrap().unwrap();
    assert_eq!(recovered_local.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(recovered_local.conversation, local_blocked.conversation);

    // A completed revision must not turn discovery into a partial, unchecked read.
    store
        .agents
        .update_one(
            doc! { "_id": applied.id.as_str() },
            doc! { "$set": { "last_seq": 999_i64 } },
        )
        .await
        .unwrap();
    assert!(matches!(
        worker.reconcile_connections(SystemTime::now()).await,
        Err(SandboxStoreError::Corrupt(_))
    ));
    client.database(&name).drop().await.unwrap();
}
