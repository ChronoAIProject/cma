//! Explicit loopback qualification; never a production capacity or canary test.
use super::*;
use crate::{
    channels::resolver::ResolvedChannel,
    delivery::{durable::DeliveryCoordinator, router::ShardedScheduler},
    http::readiness::Readiness,
    ingress::routing_store::{AdmittedPlan, PlanStore, StoredTarget},
    turns::tracker::TurnTracker,
    DeliveryConfig,
};
use futures_util::future::{join_all, BoxFuture};
use scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem};
use std::{fs::OpenOptions, io::Write, os::unix::fs::OpenOptionsExt, sync::Mutex};
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

struct Preparation {
    delay: Duration,
    input_size: usize,
}
impl FrozenTurnBuilder for Preparation {
    fn prepare<'a>(
        &'a self,
        plan: &'a AdmittedPlan,
        target: &'a StoredTarget,
        current: &'a ResolvedChannel,
    ) -> BoxFuture<'a, Result<FrozenTurn, ExecutionError>> {
        Box::pin(async move {
            tokio::time::sleep(self.delay).await;
            Ok(frozen(plan, target, current, self.input_size))
        })
    }
}

struct Runner {
    plans: PlanStore,
    journal: Journal,
    current: Arc<Current>,
    builder: Arc<Preparation>,
    observation: Duration,
    posts: Arc<std::sync::atomic::AtomicUsize>,
}
impl TargetExecutor for Runner {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            // A separate synthetic provider per execution prevents the existing
            // single-response fixture from sharing response identity across events.
            let http = HttpServer::start().await;
            let posts = self.posts.clone();
            {
                let mut state = http.memory.lock().unwrap();
                state.config.replay_delay = self.observation;
                state.config.on_post = Some(Arc::new(move || {
                    posts.fetch_add(1, Ordering::SeqCst);
                }));
            }
            let port = Arc::new(HttpExecutionPort::new(
                http.client(true, Duration::from_secs(30)),
                self.builder.clone(),
                self.plans.clone(),
                self.journal.clone(),
                self.current.clone(),
            ));
            let (tx, mut rx) = mpsc::channel(1);
            let delivery = RoutedDelivery::new(
                self.plans.clone(),
                self.journal.clone(),
                self.current.clone(),
                DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(tx))),
            );
            let executor = RoutedTargetExecutor::new(
                port,
                delivery,
                TurnTracker::new(Readiness::initialized()),
                &DeliveryConfig::default(),
            );
            let result = executor.execute(work).await;
            assert!(
                rx.try_recv().is_err(),
                "synthetic workload has no outbound delivery"
            );
            result
        })
    }
}

fn request(index: usize) -> crate::ingress::launch::LaunchRequest {
    let mut activity = fixture::custom(None);
    activity.event_id = fixture::event_id(&format!("qualification-{index}"));
    fixture::request(
        |v| {
            v["routing"]["default_targets"] = json!(["default"]);
            v["reply"]["sinks"] = json!([]);
            v["reply"]["auto_final"] = false.into();
        },
        activity,
    )
}

async fn run(mode: &str, cold: bool, media: bool) -> serde_json::Value {
    let (count, seconds, observation_ms) = match mode {
        "burst" => (100, 0, 12_000),
        "sustained" => (3600, 1800, 100),
        "smoke" => (8, 4, 100),
        _ => panic!("mode must be burst, sustained or smoke"),
    };
    let measurement = Measurement::new(4096).unwrap();
    let server = crate::ingress::routing_store::test_server::Server::start().await;
    let plans = server.store().with_start_measurement(measurement.clone());
    let mut journal = Journal::new(server.records.clone(), Arc::new(fixture::keys()));
    journal.start_measurement = Some(measurement.clone());
    let clock = server.memory.clock.clone();
    journal.clock = Arc::new(move || clock.load(Ordering::SeqCst));
    let current = Arc::new(Current {
        channel: Mutex::new(request(0).channel),
        calls: 0.into(),
        revoke_at: usize::MAX.into(),
    });
    let posts = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let prepare_ms = u64::from(cold) * 25 + u64::from(media) * 10;
    let input_bytes = if media { 4096 } else { 32 };
    let runner = Arc::new(Runner {
        plans: plans.clone(),
        journal,
        current,
        builder: Arc::new(Preparation {
            delay: Duration::from_millis(prepare_ms),
            input_size: input_bytes,
        }),
        observation: Duration::from_millis(observation_ms),
        posts: posts.clone(),
    });
    let (scheduler, handle) =
        Scheduler::new(Arc::new(plans.clone()), runner, ScheduleConfig::default()).unwrap();
    let (stop, receiver) = watch::channel(false);
    let running = tokio::spawn(scheduler.run(receiver));
    let offer_start_ns = measurement.now();
    let begin = Instant::now();
    let offers = (0..count).map(|index| {
        let plans = &plans;
        let handle = &handle;
        async move {
            if seconds != 0 {
                tokio::time::sleep_until(begin + Duration::from_millis(index as u64 * 500)).await;
            }
            let request = request(index);
            let candidate = Candidate::prepare(
                &request,
                &fixture::routed(&request),
                b"synthetic",
                &fixture::keys(),
                fixture::NOW,
            )
            .unwrap();
            let admitted = plans.admit(&candidate).await.unwrap();
            // Repeat both admission and wake-up: neither may manufacture samples or POSTs.
            let replayed = plans.admit(&candidate).await.unwrap();
            assert_eq!(replayed.admission_id, admitted.admission_id);
            handle.notify(&admitted);
            handle.notify(&replayed);
        }
    });
    join_all(offers).await;
    if seconds != 0 {
        tokio::time::sleep_until(begin + Duration::from_secs(seconds)).await;
    }
    let offer_window_ns = begin.elapsed().as_nanos() as u64;
    let budget = if mode == "burst" { 240 } else { 30 };
    let drained = tokio::time::timeout(Duration::from_secs(budget), async {
        loop {
            let snapshot = measurement.snapshot();
            if snapshot
                .samples
                .iter()
                .filter(|s| s.completed_ns.is_some())
                .count()
                == count
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    })
    .await
    .is_ok();
    stop.send(true).unwrap();
    let report = running.await.unwrap();
    let snapshot = measurement.snapshot();
    let actual_posts = posts.load(Ordering::SeqCst);
    assert_eq!(
        actual_posts as u64,
        snapshot
            .samples
            .iter()
            .map(|s| s.post_attempts)
            .sum::<u64>()
    );
    assert!(
        snapshot.samples.iter().all(|s| s.post_attempts <= 1),
        "duplicate wakeup reposted"
    );
    json!({
        "cohort": {"binding": if cold {"cold"} else {"bound"}, "media": media},
        "mode": mode, "expected_offered": count, "offer_start_ns": offer_start_ns, "offer_window_ns": offer_window_ns,
        "drained": drained, "actual_posts": actual_posts, "scheduler_aborted": report.aborted,
        "scheduler_recovery_failed": report.recovery_failed,
        "config": {"scheduler_concurrency": 8, "queue_capacity": 512,
            "preparation_delay_ms": prepare_ms, "input_bytes": input_bytes, "observation_delay_ms": observation_ms,
            "provider": "isolated_loopback_per_execution", "durable_store": "loopback_record_fixture",
            "wall_clock": "fixed_synthetic_seconds", "cohort_model": "synthetic_preparation_delay_and_payload"},
        "measurement": snapshot,
    })
}

#[tokio::test]
async fn scheduler_smoke_uses_actual_posts_and_terminal_checkpoints() {
    let row = run("smoke", false, false).await;
    assert_eq!(row["actual_posts"], 8);
    assert_eq!(row["drained"], true);
    assert_eq!(row["measurement"]["active_executors"], 0);
}

#[tokio::test]
#[ignore = "explicit local qualification; 30-minute mode is intentionally opt-in"]
async fn provider_start_local_qualification() {
    let mode = std::env::var("CMAEG_START_MODE").expect("explicit local mode");
    let path = std::env::var("CMAEG_START_OUTPUT").expect("private output path");
    let cohorts = join_all(
        [(false, false), (false, true), (true, false), (true, true)]
            .into_iter()
            .map(|(cold, media)| run(&mode, cold, media)),
    )
    .await;
    let output = json!({"schema_version": 1, "scope": "local_synthetic_only", "cohorts": cohorts});
    let mut file = OpenOptions::new()
        .write(true)
        .create_new(true)
        .mode(0o600)
        .open(path)
        .unwrap();
    file.write_all(serde_json::to_string(&output).unwrap().as_bytes())
        .unwrap();
}
