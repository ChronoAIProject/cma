//! Accepted heads with an expired reply guard must not reattach every cooldown.
use super::{mongo_progress_fixture::*, *};
use crate::{
    http::readiness::Readiness,
    replicas::partition_discovery::Frontier,
    turns::{
        routed::{
            adapter_test_server::HttpServer,
            scheduler::{Disposition, ScheduleConfig, Scheduler, TargetExecutor, WorkItem},
            tool_reply_store::{Admission, Kind, ReplyStore},
            HttpExecutionPort, RoutedDelivery, RoutedTargetExecutor, ToolReplyStatus,
        },
        TurnTracker,
    },
    DeliveryConfig,
};
use futures_util::future::BoxFuture;
use tokio::{
    sync::{mpsc, watch},
    time::Instant,
};

struct Observed {
    inner: RoutedTargetExecutor,
    settled: mpsc::UnboundedSender<Instant>,
}
impl TargetExecutor for Observed {
    fn execute<'a>(&'a self, work: &'a WorkItem) -> BoxFuture<'a, Disposition> {
        Box::pin(async move {
            let result = self.inner.execute(work).await;
            if matches!(result, Disposition::Settled) {
                let _ = self.settled.send(Instant::now());
            }
            result
        })
    }
}

#[tokio::test]
#[ignore = "requires authenticated disposable Mongo replica set; observes the real 60-second scheduler cooldown"]
async fn accepted_head_with_expired_guard_stops_futile_provider_reattach() {
    with_harness(|h| Box::pin(async move {
        let (owner, mut lease) = h.accepted().await;
        let journal = h.nodes[owner].journal();
        journal.transition(&mut lease, Phase::Accepted {
            response_id: "resp_accepted".into(), durable_cursor: None,
        }).await.unwrap();
        let guard = ReplyStore(journal.clone(), lease.partition.clone());
        let Admission::Claimed(claim) = guard.admit(&h.plan, h.target(), Kind::Tool, "a".repeat(64))
            .await.unwrap() else { panic!("one admitted tool attempt") };
        assert_eq!(guard.status(&h.plan, h.target()).await.unwrap(), ToolReplyStatus::Pending);
        journal.release(&mut lease).await.unwrap();
        let later = later_plan(h).await;
        // Lose the receipt and wait past the real 30-second claim deadline.
        // No test clock or lease constant is changed.
        tokio::time::sleep(Duration::from_secs(31)).await;
        assert!(claim.until() <= (journal.clock)());
        assert_eq!(guard.status(&h.plan, h.target()).await.unwrap(), ToolReplyStatus::Unknown);
        let http = HttpServer::start().await;
        let (settled, mut observed) = mpsc::unbounded_channel();
        let (stop, stopped) = watch::channel(false);
        let mut tasks = Vec::new();
        for (node, tools) in h.nodes.iter().zip(&h.tools) {
            let port = Arc::new(HttpExecutionPort::new(
                http.client(true, Duration::from_secs(30)), Arc::new(Builder(h.database.keys())),
                node.plans(), node.journal(), h.current.clone(),
            ));
            let delivery = RoutedDelivery::new(node.plans(), node.journal(), h.current.clone(), tools.coordinator.clone());
            let executor = Arc::new(Observed {
                inner: RoutedTargetExecutor::new(port, delivery, TurnTracker::new(Readiness::initialized()), &DeliveryConfig::default()),
                settled: settled.clone(),
            });
            let (scheduler, _) = Scheduler::new(
                Arc::new(Frontier { partitions: node.partitions.clone(), plans: node.plans() }),
                executor, ScheduleConfig {
                    concurrency: 1, queue_capacity: 8,
                    page_delay: Duration::from_millis(250),
                    sweep_delay: Duration::from_secs(1),
                    drain_timeout: Duration::from_secs(2),
                },
            ).unwrap();
            tasks.push(Task::new(tokio::spawn(scheduler.run(stopped.clone()))));
        }
        let first = tokio::time::timeout(Duration::from_secs(10), observed.recv()).await.unwrap().unwrap();
        let first_gets = http.seen().iter().filter(|request| request.method == "GET").count();
        let second = tokio::time::timeout(Duration::from_secs(66), observed.recv()).await.unwrap().unwrap();
        stop.send(true).unwrap();
        for task in tasks {
            let report = task.join().await;
            assert!(!report.recovery_failed);
            assert_eq!(report.aborted, 0);
        }
        let requests = http.seen();
        let snapshots = requests.iter().filter(|r| r.uri == "/api/v2/responses/resp_accepted").count();
        let events = requests.iter().filter(|r| r.uri.starts_with("/api/v2/responses/resp_accepted/events")).count();
        let interval = second.duration_since(first);
        assert!(interval >= Duration::from_secs(60), "must exercise the real settled cooldown");
        assert!(requests.iter().all(|r| r.method == "GET"), "recovery must not repost");
        let (state, _) = journal.read::<TargetState>(&h.target().id).await.unwrap().unwrap();
        assert!(matches!(state.phase, Phase::Accepted { .. }), "no automatic abandonment is authorized");
        assert_eq!(guard.status(&h.plan, h.target()).await.unwrap(), ToolReplyStatus::Unknown);
        assert!(h.rx.try_recv().is_err(), "uncertainty cannot authorize another answer");
        for node in &h.nodes {
            assert!(node.journal().claim(&later, &later.targets[0], "later").await.is_err(),
                "head must remain until explicit operator disposition");
        }
        assert_eq!((snapshots, events), (0, 0),
            "#2171 PR A: unchanged expired guard caused {first_gets} GETs on first visit and another pair after {} ms; head stays Accepted",
            interval.as_millis());
    })).await;
}
