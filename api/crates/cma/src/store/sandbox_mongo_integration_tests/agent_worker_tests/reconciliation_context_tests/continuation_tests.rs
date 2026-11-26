use super::*;
use crate::background::{BackgroundBudget, BackgroundWork};
use crate::sandbox::binding_lifecycle_test_support::ConnectionSweep;
use std::time::Duration;

#[path = "owner_page_tests.rs"]
mod owner_page;

#[derive(Default)]
struct PrefixProbe {
    visits: AtomicU64,
    blocked: AtomicBool,
}

impl PrefixProbe {
    async fn observe(&self) {
        self.visits.fetch_add(1, Ordering::Relaxed);
        if self.blocked.load(Ordering::Relaxed) {
            std::future::pending::<()>().await;
        }
    }
}

#[async_trait::async_trait]
impl layer3_adapter::ConversationSettingsSupport for PrefixProbe {
    async fn system_prompt_supported(&self) -> bool {
        false
    }

    async fn managed_context_may_be_ready(&self) -> bool {
        self.observe().await;
        false
    }

    async fn managed_transport_receipt(&self, _: u64) -> Option<String> {
        self.observe().await;
        Some("a".repeat(64))
    }
}

async fn attached_prefix(store: &MongoSandboxStore, suffix: &str) -> Agent {
    let mut agent = idle_connection_fixture(store, suffix, AgentStatus::Ready).await;
    let mut settings = agent.settings();
    settings.model = "fixture-model".into();
    agent.conversation.settings = Some(settings);
    agent.revision += 1;
    replace_agent_fixture(store, &agent).await;
    agent
}

async fn paused_rows(store: &MongoSandboxStore, count: usize) {
    let base = idle_connection_fixture(store, "100_fill", AgentStatus::Paused).await;
    let rows = (1..count).map(|index| {
        let mut agent = base.clone();
        let suffix = format!("100_fill_{index:03}");
        agent.id = AgentId(format!("agt_agent_work_{suffix}"));
        agent.worktree_id = WorktreeId(format!("wkt_agent_work_{suffix}"));
        agent.creation_key = format!("create-agent-work-{suffix}");
        crate::store::agents::agent_document(&agent).unwrap()
    });
    store.agents.insert_many(rows).await.unwrap();
}

fn sliced_work(budget: Arc<BackgroundBudget>, deadline: Duration) -> BackgroundWork {
    BackgroundWork::new(
        budget,
        "agent_reconciliation_test",
        deadline,
        Duration::ZERO,
    )
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn agent_reconciliation_continuation_survives_cancellation_and_wraps() {
    let (client, name) = database("connection_continuation").await;
    let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (store, owner) = manager(&fixture, "continuation-owner").await;
    let registry = Arc::new(LiveAgentRegistry::default());
    let manager = worker(store.clone(), owner.clone(), registry.clone(), true);
    let first = attached_prefix(&fixture, "001_first").await;
    let slow = attached_prefix(&fixture, "002_slow").await;
    paused_rows(&fixture, 201).await;
    let tail = idle_connection_fixture(&fixture, "900_tail", AgentStatus::Ready).await;
    let first_probe = Arc::new(PrefixProbe::default());
    let slow_probe = Arc::new(PrefixProbe::default());
    slow_probe.blocked.store(true, Ordering::Relaxed);
    let _senders = [(&first, &first_probe), (&slow, &slow_probe)].map(|(agent, probe)| {
        attach(
            &registry,
            agent,
            0,
            true,
            ConversationInputSender::spawn(1, |_| async { Ok(()) })
                .with_settings_support(probe.clone()),
        )
    });
    let now = SystemTime::now();
    let budget = BackgroundBudget::with_concurrency(1);
    let deadline = Duration::from_secs(1);
    for expected in 1..=2 {
        let full = sliced_work(budget.clone(), deadline);
        assert!(full
            .run(|| manager.reconcile_connections(now))
            .await
            .is_err());
        assert_eq!(first_probe.visits.load(Ordering::Relaxed), expected);
        assert_eq!(slow_probe.visits.load(Ordering::Relaxed), expected);
        assert_eq!(
            fixture.get_agent(&tail.id).await.unwrap().as_ref(),
            Some(&tail)
        );
    }

    let mut sweep = ConnectionSweep::default();
    let first_slice = sliced_work(budget.clone(), deadline);
    assert!(first_slice
        .run_resumable(|| manager.reconcile_connection_slice(now, &mut sweep))
        .await
        .is_err());
    assert_eq!(sweep.visited(), (1, 2));
    assert_eq!(first_probe.visits.load(Ordering::Relaxed), 3);
    let inserted = idle_connection_fixture(&fixture, "000_inserted", AgentStatus::Ready).await;
    slow_probe.blocked.store(false, Ordering::Relaxed);
    let foreground = budget.foreground();
    let probe = sliced_work(budget.clone(), deadline);
    assert_eq!(probe.run(|| async { Ok(()) }).await.unwrap(), Some(()));
    drop(foreground);

    let started = std::time::Instant::now();
    let mut slices = 0;
    loop {
        slices += 1;
        assert!(slices <= 8, "bounded pages must reach the tail");
        let previous = sweep.visited();
        let slice = sliced_work(budget.clone(), deadline);
        match slice
            .run_resumable(|| manager.reconcile_connection_slice(now, &mut sweep))
            .await
            .unwrap()
        {
            Some(true) => break,
            Some(false) => {
                let next = sweep.visited();
                assert_eq!(next.0, previous.0 + 1);
                assert!(next.1 > previous.1 && next.1 <= previous.1 + 32);
            }
            None => panic!("the prior slice retained its shared permit"),
        }
    }
    let saved = fixture.get_agent(&tail.id).await.unwrap().unwrap();
    assert_eq!(saved.revision, tail.revision + 1);
    assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(first_probe.visits.load(Ordering::Relaxed), 3);
    assert_eq!(slow_probe.visits.load(Ordering::Relaxed), 4);
    assert_eq!(
        fixture.get_agent(&inserted.id).await.unwrap().as_ref(),
        Some(&inserted)
    );
    eprintln!("baseline_prefix_visits=2 cancelled_page=1 cancelled_page_rows=2 resumed_pages={slices} candidate_cap=32 tail_ms={} foreground_admissions_between_slices=1", started.elapsed().as_millis());

    assert!(!manager
        .reconcile_connection_slice(now, &mut sweep)
        .await
        .unwrap());
    assert_eq!(
        fixture
            .get_agent(&inserted.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        inserted.revision + 1
    );
    let restart_target =
        idle_connection_fixture(&fixture, "000_after_restart", AgentStatus::Ready).await;
    let restarted = worker(store, owner, registry, true);
    assert!(!restarted
        .reconcile_connection_slice(now, &mut ConnectionSweep::default())
        .await
        .unwrap());
    assert_eq!(
        fixture
            .get_agent(&restart_target.id)
            .await
            .unwrap()
            .unwrap()
            .revision,
        restart_target.revision + 1
    );
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn agent_reconciliation_continuation_revisits_prefix_after_repeated_stalls() {
    let (client, name) = database("connection_continuation_stall").await;
    let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
    let (store, owner) = manager(&fixture, "continuation-stall-owner").await;
    let registry = Arc::new(LiveAgentRegistry::default());
    let worker = worker(store, owner.clone(), registry.clone(), true);
    let first = attached_prefix(&fixture, "001_first").await;
    let blocked = attached_prefix(&fixture, "002_blocked").await;
    paused_rows(&fixture, 32).await;
    let tail = idle_connection_fixture(&fixture, "900_tail", AgentStatus::Ready).await;
    let first_probe = Arc::new(PrefixProbe::default());
    let blocked_probe = Arc::new(PrefixProbe::default());
    blocked_probe.blocked.store(true, Ordering::Relaxed);
    let _senders = [(&first, &first_probe), (&blocked, &blocked_probe)].map(|(agent, probe)| {
        attach(
            &registry,
            agent,
            0,
            true,
            ConversationInputSender::spawn(1, |_| async { Ok(()) })
                .with_settings_support(probe.clone()),
        )
    });
    let budget = BackgroundBudget::with_concurrency(1);
    let mut sweep = ConnectionSweep::default();
    let work = sliced_work(budget.clone(), Duration::from_secs(1));
    assert!(work
        .run_resumable(|| worker.reconcile_connection_slice(SystemTime::now(), &mut sweep))
        .await
        .is_err());
    assert_eq!(first_probe.visits.load(Ordering::Relaxed), 1);
    let inserted = idle_connection_fixture(&fixture, "000_inserted", AgentStatus::Ready).await;

    for expected in 2..=3 {
        owner.maintain().await.unwrap();
        let started = tokio::time::Instant::now();
        let next = work.next_run_at();
        assert!(started < next, "a stalled admission must retain backoff");
        assert!(work
            .run_resumable(|| worker.reconcile_connection_slice(SystemTime::now(), &mut sweep))
            .await
            .is_err());
        assert!(tokio::time::Instant::now() >= next);
        assert_eq!(first_probe.visits.load(Ordering::Relaxed), 1);
        assert_eq!(blocked_probe.visits.load(Ordering::Relaxed), expected);
        assert_eq!(
            fixture.get_agent(&inserted.id).await.unwrap().as_ref(),
            Some(&inserted)
        );
    }

    owner.maintain().await.unwrap();
    assert!(work
        .run_resumable(|| worker.reconcile_connection_slice(SystemTime::now(), &mut sweep))
        .await
        .is_err());
    assert_eq!(first_probe.visits.load(Ordering::Relaxed), 2);
    assert_eq!(blocked_probe.visits.load(Ordering::Relaxed), 4);
    let saved = fixture.get_agent(&inserted.id).await.unwrap().unwrap();
    assert_eq!(saved.revision, inserted.revision + 1);
    assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
    let owner = client
        .database(&name)
        .collection::<Document>(crate::store::agent_ownership::OWNERS)
        .find_one(doc! {"_id": inserted.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("effect_revision").unwrap(), 1);
    assert_eq!(
        fixture.get_agent(&tail.id).await.unwrap().as_ref(),
        Some(&tail)
    );

    blocked_probe.blocked.store(false, Ordering::Relaxed);
    let resume = sliced_work(budget, Duration::from_secs(2));
    assert_eq!(
        resume
            .run_resumable(|| worker.reconcile_connection_slice(SystemTime::now(), &mut sweep))
            .await
            .unwrap(),
        Some(false)
    );
    assert_eq!(
        resume
            .run_resumable(|| worker.reconcile_connection_slice(SystemTime::now(), &mut sweep))
            .await
            .unwrap(),
        Some(true)
    );
    assert_eq!(
        fixture.get_agent(&tail.id).await.unwrap().unwrap().revision,
        tail.revision + 1
    );
    assert_eq!(fixture.get_agent(&inserted.id).await.unwrap(), Some(saved));
    eprintln!("stalled_admissions=2 prefix_recoveries_while_blocked=1 blocker_visits=4 tail_recoveries_after_release=1");
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn agent_reconciliation_continuation_fences_one_tail_recovery_across_managers() {
    let (client, name) = database("connection_continuation_race").await;
    let fixture = MongoSandboxStore::open(&client, &name).await.unwrap();
    paused_rows(&fixture, 64).await;
    let mut tail = idle_connection_fixture(&fixture, "900_race", AgentStatus::Paused).await;
    let second_client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
        .await
        .unwrap();
    let second = MongoSandboxStore::open(&second_client, &name)
        .await
        .unwrap();
    let (left, left_owner) = manager(&fixture, "left-continuation").await;
    let (right, right_owner) = manager(&second, "right-continuation").await;
    let left = worker(
        left,
        left_owner,
        Arc::new(LiveAgentRegistry::default()),
        true,
    );
    let right = worker(
        right,
        right_owner,
        Arc::new(LiveAgentRegistry::default()),
        true,
    );
    let (mut left_sweep, mut right_sweep) =
        (ConnectionSweep::default(), ConnectionSweep::default());
    let now = SystemTime::now();
    for _ in 0..2 {
        let (left, right) = tokio::join!(
            left.reconcile_connection_slice(now, &mut left_sweep),
            right.reconcile_connection_slice(now, &mut right_sweep)
        );
        assert!(!left.unwrap() && !right.unwrap());
    }
    assert_eq!(left_sweep.visited(), (2, 64));
    assert_eq!(right_sweep.visited(), (2, 64));
    tail.status = AgentStatus::Ready;
    tail.revision += 1;
    replace_agent_fixture(&fixture, &tail).await;
    let started = std::time::Instant::now();
    let (left, right) = tokio::join!(
        left.reconcile_connection_slice(now, &mut left_sweep),
        right.reconcile_connection_slice(now, &mut right_sweep)
    );
    assert!(left.unwrap() && right.unwrap());
    let saved = fixture.get_agent(&tail.id).await.unwrap().unwrap();
    assert_eq!(saved.revision, tail.revision + 1);
    assert_eq!(saved.work.step, AgentWorkStep::PublishConnection);
    assert_eq!(saved.conversation, tail.conversation);
    let owner = client
        .database(&name)
        .collection::<Document>(crate::store::agent_ownership::OWNERS)
        .find_one(doc! {"_id": tail.id.as_str()})
        .await
        .unwrap()
        .unwrap();
    assert_eq!(owner.get_i64("effect_revision").unwrap(), 1);
    eprintln!("replicas=2 pages_per_replica=3 rows_per_replica=65 tail_ms={} fenced_recovery_publications=1", started.elapsed().as_millis());
    client.database(&name).drop().await.unwrap();
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn agent_reconciliation_continuation_keeps_transport_limit_after_cancellation() {
    let (client, name) = database("connection_transport_continuation").await;
    let store = Arc::new(MongoSandboxStore::open(&client, &name).await.unwrap());
    let registry = Arc::new(LiveAgentRegistry::default());
    let first = attached_prefix(&store, "001_migrate").await;
    paused_rows(&store, 32).await;
    let second = attached_prefix(&store, "200_migrate").await;
    let third = attached_prefix(&store, "201_migrate").await;
    let probes = [(); 3].map(|_| Arc::new(PrefixProbe::default()));
    probes[0].blocked.store(true, Ordering::Relaxed);
    let _senders: Vec<_> = [&first, &second, &third]
        .into_iter()
        .zip(&probes)
        .map(|(agent, probe)| {
            attach(
                &registry,
                agent,
                0,
                true,
                ConversationInputSender::spawn(1, |_| async { Ok(()) })
                    .with_settings_support(probe.clone()),
            )
        })
        .collect();
    let worker = AgentWorker::new(
        AgentWorkerDependencies {
            store: store.clone(),
            adapter: Arc::new(CodexAdapter::with_backend(
                InMemoryHostedBackend::default(),
                CodexSandboxSpec::default(),
            )),
            skills: Arc::new(EmptyAgentSkillResolver),
            registry: registry.clone(),
            updates: DurableAgentUpdateRecorder::new(store.clone()),
            operations: AgentOperationService::new(store.clone(), registry),
        },
        AgentWorkerConfig::new("continuation-migration", "continuation-migration")
            .with_managed_documents(true)
            .with_access_transport_migration(Some(crate::config::AgentAccessMigration {
                revision: 2,
                origin: "https://new.example".into(),
                agents: [&first, &second, &third]
                    .map(|agent| agent.id.to_string())
                    .to_vec(),
            })),
    );
    let now = SystemTime::now();
    let mut sweep = ConnectionSweep::default();
    assert!(tokio::time::timeout(
        Duration::from_secs(1),
        worker.reconcile_connection_slice(now, &mut sweep),
    )
    .await
    .is_err());
    assert_eq!(probes[0].visits.load(Ordering::Relaxed), 1);
    probes[0].blocked.store(false, Ordering::Relaxed);
    assert!(!worker
        .reconcile_connection_slice(now, &mut sweep)
        .await
        .unwrap());
    assert!(worker
        .reconcile_connection_slice(now, &mut sweep)
        .await
        .unwrap());
    assert_eq!(probes[0].visits.load(Ordering::Relaxed), 1);
    assert_eq!(probes[1].visits.load(Ordering::Relaxed), 1);
    assert_eq!(probes[2].visits.load(Ordering::Relaxed), 0);
    assert!(!worker
        .reconcile_connection_slice(now, &mut sweep)
        .await
        .unwrap());
    assert!(worker
        .reconcile_connection_slice(now, &mut sweep)
        .await
        .unwrap());
    assert_eq!(probes[2].visits.load(Ordering::Relaxed), 1);
    client.database(&name).drop().await.unwrap();
}
