use super::*;
use std::collections::BTreeMap;
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

// These attached streams emit no updates. Keep their asynchronous checkpoint
// restoration separate from the reconciliation reads measured by this test.
struct IdleRecorder;

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
async fn applied_context_skips_duplicate_reads_but_pending_refresh_and_recovery_still_read() {
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
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let mut applied = idle_connection_fixture(&store, "applied_context", AgentStatus::Ready).await;
    applied.conversation.managed_guidance_revision = 2;
    applied.revision += 1;
    replace_agent_fixture(&store, &applied).await;
    let pending = idle_connection_fixture(&store, "pending_context", AgentStatus::Ready).await;
    let mut detached =
        idle_connection_fixture(&store, "detached_context", AgentStatus::Ready).await;
    detached.conversation.managed_guidance_revision = 2;
    detached.revision += 1;
    replace_agent_fixture(&store, &detached).await;

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
    for agent in [&applied, &pending] {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        senders.push(sender);
        let attachment = registry
            .attach(
                agent.id.clone(),
                agent.conversation.conversation_id.clone().unwrap(),
                agent.conversation.runtime_generation.unwrap(),
                agent.work.next_fence - 1,
                RuntimeCapabilities::new([RuntimeCapability::Conversation]),
                ConversationInputSender::spawn(1, |_| async { Ok(()) }),
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
        1
    );
    let observed = reads.lock().unwrap().clone();
    assert_eq!(observed.get(applied.id.as_str()), None);
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
