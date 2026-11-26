use super::*;
use layer2_core::agent_ownership::{
    Member, OwnershipCoordinator, OwnershipStore, PROTOCOL_VERSION,
};
use layer3_adapter::ConversationUpdate;
use mongodb::event::cmap::CmapEvent;
use tokio::sync::mpsc;

#[path = "continuation_tests.rs"]
mod continuation;

#[derive(Default)]
struct MaintenanceProbe(AtomicU64);

#[async_trait::async_trait]
impl layer3_adapter::ConversationSettingsSupport for MaintenanceProbe {
    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn managed_context_may_be_ready(&self) -> bool {
        assert!(layer2_core::agent_ownership::execution_owner().is_some());
        self.0.fetch_add(1, Ordering::Relaxed);
        true
    }
}

async fn manager(
    store: &MongoSandboxStore,
    id: &str,
) -> (Arc<MongoSandboxStore>, Arc<OwnershipCoordinator>) {
    let store = Arc::new(
        store
            .clone()
            .enable_agent_ownership(id.into())
            .await
            .unwrap(),
    );
    let coordinator = OwnershipCoordinator::register(
        store.agent_ownership.as_ref().unwrap().clone(),
        Member {
            instance_id: id.into(),
            protocol: PROTOCOL_VERSION,
            relay_address: "http://fixture.invalid:8080".into(),
            expires_at_ms: 0,
            draining: false,
            owned_agents: 0,
            max_owned_agents: 32,
        },
    )
    .await
    .unwrap();
    (store, coordinator)
}

fn worker(
    store: Arc<MongoSandboxStore>,
    coordinator: Arc<OwnershipCoordinator>,
    registry: Arc<LiveAgentRegistry>,
    skip: bool,
) -> Arc<AgentWorker> {
    configured_worker(
        store,
        registry,
        AgentWorkerConfig::new("reconcile-ownership", "reconcile-ownership")
            .with_ownership(Some(coordinator))
            .with_skip_ineligible_reconciliation(skip)
            .with_managed_documents(true),
    )
}

fn configured_worker(
    store: Arc<MongoSandboxStore>,
    registry: Arc<LiveAgentRegistry>,
    config: AgentWorkerConfig,
) -> Arc<AgentWorker> {
    AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store, registry.clone()),
            registry,
        },
        config,
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn foreign_ownership_deferral_saves_checkouts_and_rechecks_after_handoff() {
    let (_, name) = database("reconcile_foreign_owner").await;
    let uri = std::env::var("CHRONO_TEST_MONGODB_URI").unwrap();
    let checkouts = Arc::new(AtomicU64::new(0));
    let mut options = ClientOptions::parse(&uri).await.unwrap();
    let observed = checkouts.clone();
    options.cmap_event_handler = Some(EventHandler::callback(move |event| {
        if matches!(event, CmapEvent::ConnectionCheckedOut(_)) {
            observed.fetch_add(1, Ordering::Relaxed);
        }
    }));
    let client = Client::with_options(options).unwrap();
    let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = idle_connection_fixture(&fixture, "foreign", AgentStatus::Paused).await;
    let (left_store, left) = manager(&fixture, "left").await;
    let owner = left.acquire(&agent.id).await.unwrap().unwrap();
    for index in 0..7 {
        let name = format!("foreign_peer_{index}");
        let peer = idle_connection_fixture(&fixture, &name, AgentStatus::Paused).await;
        left.acquire(&peer.id).await.unwrap().unwrap();
    }
    let build = |store: Arc<MongoSandboxStore>, coordinator: Arc<OwnershipCoordinator>, enabled| {
        configured_worker(
            store,
            Arc::new(LiveAgentRegistry::default()),
            AgentWorkerConfig::new("reconcile-foreign-owner", "reconcile-foreign-owner")
                .with_ownership(Some(coordinator))
                .with_defer_foreign_reconciliation(enabled),
        )
    };
    let left_baseline = build(left_store.clone(), left.clone(), false);
    let left_enabled = build(left_store, left.clone(), true);
    let measure = |worker: Arc<AgentWorker>| {
        let checkouts = checkouts.clone();
        async move {
            let before = checkouts.load(Ordering::Relaxed);
            assert_eq!(
                worker
                    .reconcile_connections(SystemTime::now())
                    .await
                    .unwrap(),
                0
            );
            checkouts.load(Ordering::Relaxed) - before
        }
    };
    let single_baseline = measure(left_baseline.clone()).await;
    let single_enabled = measure(left_enabled.clone()).await;
    assert_eq!(single_enabled, single_baseline);

    let (right_store, right) = manager(&fixture, "right").await;
    let right_baseline = build(right_store.clone(), right.clone(), false);
    let right_enabled = build(right_store.clone(), right.clone(), true);
    let paired_baseline = measure(left_baseline).await + measure(right_baseline).await;
    let paired_enabled = measure(left_enabled).await + measure(right_enabled.clone()).await;
    assert_eq!(paired_enabled, single_enabled + 2);
    assert!(paired_baseline >= paired_enabled + 3);
    println!("reconciliation checkout count N=1 {single_baseline}->{single_enabled} N=2 {paired_baseline}->{paired_enabled}");
    assert_eq!(
        right.store.owner(&agent.id).await.unwrap().unwrap().token,
        owner.token
    );
    assert_eq!(
        fixture.get_agent(&agent.id).await.unwrap().as_ref(),
        Some(&agent)
    );

    agent.status = AgentStatus::Ready;
    agent.revision += 1;
    replace_agent_fixture(&fixture, &agent).await;
    assert_eq!(measure(right_enabled.clone()).await, 2);
    assert_eq!(
        fixture.get_agent(&agent.id).await.unwrap().as_ref(),
        Some(&agent)
    );

    left.begin_drain().await;
    left.handoff(&owner, async { Ok(()) }).await.unwrap();
    let now = SystemTime::now();
    assert_eq!(right_enabled.reconcile_connections(now).await.unwrap(), 1);
    assert_eq!(right_enabled.reconcile_connections(now).await.unwrap(), 0);
    let saved = fixture.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(saved.revision, agent.revision + 1);
    assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
    let successor = right.store.owner(&agent.id).await.unwrap().unwrap();
    assert_eq!(successor.token.instance_id, "right");
    assert!(successor.token.epoch > owner.token.epoch);
    client.database(&name).drop().await.unwrap();
}

fn attach(
    registry: &Arc<LiveAgentRegistry>,
    agent: &Agent,
    generation_offset: u64,
    published: bool,
    input: ConversationInputSender,
) -> mpsc::Sender<ConversationUpdate> {
    let (sender, receiver) = mpsc::channel(1);
    let attachment = registry
        .attach(
            agent.id.clone(),
            agent.conversation.conversation_id.clone().unwrap(),
            agent.conversation.runtime_generation.unwrap() + generation_offset,
            agent.work.next_fence - 1,
            RuntimeCapabilities::new([RuntimeCapability::Conversation]),
            input,
            receiver,
            Arc::new(IdleRecorder),
        )
        .unwrap();
    if published {
        registry.publish_and_activate(&attachment).unwrap();
    }
    sender
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ownership_skip_uses_exact_staged_and_published_targets_and_keeps_default() {
    let (client, name) = database("ownership_skip").await;
    let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (store, authority) = manager(&fixture, "owner").await;
    let ownership = store.agent_ownership.as_ref().unwrap();
    let registry = Arc::new(LiveAgentRegistry::default());
    let staged = idle_connection_fixture(&fixture, "staged", AgentStatus::Paused).await;
    let published = idle_connection_fixture(&fixture, "published", AgentStatus::Paused).await;
    let detached = idle_connection_fixture(&fixture, "detached", AgentStatus::Paused).await;
    let stale = idle_connection_fixture(&fixture, "mismatched", AgentStatus::Paused).await;
    let mut maintained = idle_connection_fixture(&fixture, "maintained", AgentStatus::Ready).await;
    let mut settings = maintained.settings();
    settings.model = "fixture-model".into();
    maintained.conversation.settings = Some(settings);
    maintained.revision += 1;
    replace_agent_fixture(&fixture, &maintained).await;
    let input = || ConversationInputSender::spawn(1, |_| async { Ok(()) });
    let maintenance = Arc::new(MaintenanceProbe::default());
    let _senders = [
        attach(&registry, &staged, 0, false, input()),
        attach(&registry, &published, 0, true, input()),
        attach(&registry, &stale, 1, true, input()),
        attach(
            &registry,
            &maintained,
            0,
            true,
            input().with_settings_support(maintenance.clone()),
        ),
    ];
    let enabled = worker(store.clone(), authority.clone(), registry.clone(), true);
    assert_eq!(
        enabled
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert_eq!(maintenance.0.load(Ordering::Relaxed), 1);
    for agent in [&staged, &published] {
        assert!(ownership.has_owner_history(&agent.id).await.unwrap());
    }
    for agent in [&detached, &stale] {
        assert!(!ownership.has_owner_history(&agent.id).await.unwrap());
    }

    let disabled = worker(store.clone(), authority, registry, false);
    assert_eq!(
        disabled
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    for agent in [&detached, &stale] {
        assert!(ownership.has_owner_history(&agent.id).await.unwrap());
        assert_eq!(
            store.get_agent(&agent.id).await.unwrap().as_ref(),
            Some(agent)
        );
    }
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn ownership_skip_reloads_an_agent_changed_without_a_wake_and_fences_one_recovery() {
    let (client, name) = database("ownership_recovery").await;
    let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
    let mut agent = idle_connection_fixture(&fixture, "missed_wake", AgentStatus::Paused).await;
    let (left_store, left_manager) = manager(&fixture, "left").await;
    let (right_store, right_manager) = manager(&fixture, "right").await;
    let left = worker(
        left_store.clone(),
        left_manager,
        Arc::new(LiveAgentRegistry::default()),
        true,
    );
    let right = worker(
        right_store,
        right_manager,
        Arc::new(LiveAgentRegistry::default()),
        true,
    );
    let ownership = left_store.agent_ownership.as_ref().unwrap();
    assert_eq!(
        left.reconcile_connections(SystemTime::now()).await.unwrap(),
        0
    );
    assert_eq!(
        right
            .reconcile_connections(SystemTime::now())
            .await
            .unwrap(),
        0
    );
    assert!(!ownership.has_owner_history(&agent.id).await.unwrap());

    // A write from another process cannot deliver this process's local wake.
    agent.status = AgentStatus::Ready;
    agent.revision += 1;
    replace_agent_fixture(&fixture, &agent).await;
    let now = SystemTime::now();
    let (first, second) = tokio::join!(
        left.reconcile_connections(now),
        right.reconcile_connections(now)
    );
    assert_eq!(first.unwrap() + second.unwrap(), 1);
    let saved = fixture.get_agent(&agent.id).await.unwrap().unwrap();
    assert_eq!(saved.revision, agent.revision + 1);
    assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(saved.conversation, agent.conversation);
    let owner = client
        .database(&name)
        .collection::<Document>(crate::store::agent_ownership::OWNERS)
        .find_one(doc! {"_id": agent.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("effect_revision").unwrap(), 1);
    let (again_left, again_right) = tokio::join!(
        left.reconcile_connections(now),
        right.reconcile_connections(now)
    );
    assert_eq!(again_left.unwrap() + again_right.unwrap(), 0);
    assert_eq!(fixture.get_agent(&agent.id).await.unwrap(), Some(saved));
    client.database(&name).drop().await.unwrap();
}
