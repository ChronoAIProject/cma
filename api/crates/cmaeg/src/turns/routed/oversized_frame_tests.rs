use super::{
    adapter_test_server::HttpServer,
    scheduler::{Disposition, TargetExecutor, WorkItem},
    *,
};
use crate::{
    activity::{ActivityStore, ConsumerState},
    channels::resolver::ChannelResolver,
    delivery::{
        custom::CustomDelivery, durable::DeliveryCoordinator, engine::DeliveryEngine,
        relay_client::RelayClient, router::ShardedScheduler,
    },
    http::readiness::Readiness,
    ingress::{
        routing_plan::{fixture, plan, OrdinaryPlan, RoutingOptions},
        routing_store::{
            test_server::{Row, Server},
            AdmittedPlan, Candidate,
        },
    },
    turns::TurnTracker,
    DeliveryConfig,
};
use cmaeg_core::seal::seal_blob;
use serde_json::{json, Value};
use std::{
    sync::{atomic::Ordering, Arc},
    time::Duration,
};

// Real authority, preparation, executor, delivery and journal implementations;
// only the external record service and provider are loopback HTTP fixtures.
struct Fixture {
    records: Server,
    plan: AdmittedPlan,
    journal: Journal,
    activity: Arc<ActivityStore>,
}
impl Fixture {
    async fn new() -> Self {
        let mut request = fixture::request(
            |value| {
                value["record_id"] = uuid::Uuid::new_v4().to_string().into();
                value["provider"]["target"] =
                    json!({"type":"existing_agent","agent_id":"agt_native"});
                value["routing"]["consumers"] = json!([]);
                value["secrets"]["consumer_keys"] = json!([]);
                value["reply"]["sinks"] = json!([]);
                value["reply"]["auto_final"] = true.into();
            },
            fixture::custom(None),
        );
        let records = Server::start().await;
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        records.memory.clock.store(now, Ordering::SeqCst);
        let channel = Arc::make_mut(&mut request.channel);
        let record = channel.definition.record_id.to_string();
        channel.pointer.record_url = records.records.record_url(&record).unwrap().to_string();
        channel.id = channel.pointer.seal_id(&fixture::keys()).unwrap();
        let channel_id = channel.id.clone();
        records.memory.channels.lock().unwrap().insert(
            record,
            Row {
                owner: channel.definition.owner.subject.clone(),
                blob: seal_blob(
                    &fixture::keys(),
                    &serde_json::to_vec(&channel.definition.to_blob_json()).unwrap(),
                )
                .unwrap(),
                expiry: now + 86400,
                version: 1,
            },
        );
        fixture::rewrite_ref(&mut request, |event| {
            event.channel_id = channel_id;
            event.expires_at = now + 900;
        });
        let OrdinaryPlan::Routed(routed) = plan(
            &request,
            &fixture::keys(),
            now,
            RoutingOptions {
                enabled: true,
                attributes_signed: true,
                non_human_providers: &std::collections::BTreeSet::from(["cma_codex".into()]),
            },
        )
        .unwrap() else {
            panic!("routed admission")
        };
        let candidate =
            Candidate::prepare(&request, &routed, b"raw-event", &fixture::keys(), now).unwrap();
        let plan = records.store().admit(&candidate).await.unwrap();
        let activity = Arc::new(ActivityStore::new(
            records.records.clone(),
            Arc::new(fixture::keys()),
        ));
        activity.received(&request).await.unwrap();
        let mut journal = Journal::new(records.records.clone(), Arc::new(fixture::keys()))
            .with_activity(activity.clone());
        let clock = records.memory.clock.clone();
        journal.clock = Arc::new(move || clock.load(Ordering::SeqCst));
        Self {
            records,
            plan,
            journal,
            activity,
        }
    }

    fn port(&self, http: &HttpServer) -> Arc<HttpExecutionPort> {
        let client = http.client(true, Duration::from_secs(5));
        Arc::new(HttpExecutionPort::new(
            client.clone(),
            Arc::new(ProductionFrozenTurnBuilder::new(
                client,
                Arc::new(fixture::keys()),
            )),
            self.records.store(),
            self.journal.clone(),
            self.authority(),
        ))
    }

    fn authority(&self) -> Arc<dyn Authority> {
        Arc::new(ChannelResolver::new(
            Arc::new(self.records.records.clone()),
            Arc::new(fixture::keys()),
            Default::default(),
        ))
    }

    fn executor(&self, http: &HttpServer) -> (RoutedTargetExecutor, DeliveryEngine, WorkItem) {
        let (sender, receiver) = tokio::sync::mpsc::channel(1);
        let delivery = RoutedDelivery::new(
            self.records.store(),
            self.journal.clone(),
            self.authority(),
            DeliveryCoordinator::new(Arc::new(ShardedScheduler::new(sender))),
        );
        let tracker = TurnTracker::new(Readiness::initialized());
        let engine = DeliveryEngine::start(
            receiver,
            Arc::new(fixture::keys()),
            RelayClient::new(&http.base, Duration::from_secs(5)).unwrap(),
            DeliveryConfig::default(),
            tracker.clone(),
            CustomDelivery::new(http.base.clone(), Arc::default()),
        );
        let runner = RoutedTargetExecutor::new(
            self.port(http),
            delivery,
            tracker,
            &DeliveryConfig::default(),
        );
        (
            runner,
            engine,
            WorkItem {
                identity: self.plan.identity.clone(),
                target_id: self.plan.targets[0].id.clone(),
            },
        )
    }

    async fn stored(&self) -> TargetState {
        self.journal
            .read::<TargetState>(&self.plan.targets[0].id)
            .await
            .unwrap()
            .unwrap()
            .0
    }

    async fn consumer(&self) -> crate::activity::ActivityConsumer {
        let identity = &self.plan.identity;
        self.activity
            .latest(&identity.owner, identity.record_id.as_str())
            .await
            .unwrap()
            .items[0]
            .consumers[0]
            .clone()
    }
}

fn frames() -> Vec<Value> {
    let mut frames = super::executor_tests::frames();
    // A completed response repeats tool output as well as the short final answer.
    // Only its digest belongs in the delivery checkpoint, not the tool body.
    frames[3]["response"]["output"] = json!([
        {"type":"function_call_output","output":"x".repeat(265_883)},
        {"type":"message","content":[{"type":"output_text","text":"hello world"}]}
    ]);
    frames[3]["sequence_number"] = 4.into();
    frames.insert(3, json!({"type":"response.output_text.done","sequence_number":3,"item_id":"m","text":"hello world"}));
    let size = serde_json::to_vec(&frames[4]).unwrap().len();
    assert!(size > 256 * 1024 && size < 1024 * 1024);
    frames
}

async fn provider() -> HttpServer {
    let http = HttpServer::start().await;
    {
        let mut memory = http.memory.lock().unwrap();
        memory.config.replay_frames = Some(frames());
        memory.config.mirror_replay_metadata = true;
    }
    http
}

async fn completed_once(
    fixture: &Fixture,
    http: &HttpServer,
    runner: &RoutedTargetExecutor,
    work: &WorkItem,
) {
    let terminal = fixture.stored().await;
    assert!(matches!(
        terminal.phase,
        Phase::Completed {
            durable_cursor: Some(4),
            ..
        }
    ));
    let consumer = fixture.consumer().await;
    assert_eq!(consumer.state, ConsumerState::Completed);
    assert!(consumer.completed_at.is_some());
    assert_eq!(consumer.response_id.as_deref(), Some("resp_accepted"));
    let requests = http.seen().len();
    for _ in 0..2 {
        assert!(matches!(runner.execute(work).await, Disposition::Settled));
        let replay = fixture.stored().await;
        assert_eq!(replay.phase, terminal.phase);
        assert_eq!(
            serde_json::to_value(replay.delivery).unwrap(),
            serde_json::to_value(&terminal.delivery).unwrap()
        );
        assert_eq!(fixture.consumer().await.completed_at, consumer.completed_at);
    }
    assert_eq!(http.seen().len(), requests);
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );
    assert!(
        http.seen()
            .iter()
            .all(|request| request.uri.starts_with("/api/v2/responses")
                || (request.method == "PUT"
                    && request.uri.starts_with("/bindings/")
                    && request.uri.contains("/conversations/"))),
        "empty sinks must not produce outbound platform requests"
    );
}

#[tokio::test]
async fn oversized_terminal_completes_journal_and_activity_once_without_sinks() {
    let fixture = Fixture::new().await;
    let http = provider().await;
    let (runner, _engine, work) = fixture.executor(&http);
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    completed_once(&fixture, &http, &runner, &work).await;
}

#[tokio::test]
async fn oversized_terminal_completes_once_with_expired_custom_event_ref() {
    let fixture = Fixture::new().await;
    let http = provider().await;
    let expires = fixture.plan.targets[0].launch.event.expires_at;
    fixture
        .records
        .memory
        .clock
        .store(expires - 1, Ordering::SeqCst);
    let clock = fixture.records.memory.clock.clone();
    http.memory.lock().unwrap().config.on_snapshot = Some(Arc::new(move || {
        clock.store(expires, Ordering::SeqCst);
    }));
    let (runner, _engine, work) = fixture.executor(&http);
    assert!(matches!(runner.execute(&work).await, Disposition::Settled));
    completed_once(&fixture, &http, &runner, &work).await;
}

#[tokio::test]
async fn retained_oversized_terminal_projection_recovers_after_expiry_without_resubmission() {
    let mut fixture = Fixture::new().await;
    let http = provider().await;
    let (mut first, engine, work) = fixture.executor(&http);
    first.frame_limit = 4;
    assert!(matches!(first.execute(&work).await, Disposition::Retry(_)));
    assert!(matches!(
        fixture.stored().await.phase,
        Phase::Accepted {
            durable_cursor: Some(3),
            ..
        }
    ));
    let produced = fixture.consumer().await;
    assert_eq!(produced.state, ConsumerState::Produced);
    assert!(produced.completed_at.is_none());
    drop(first);
    drop(engine);

    // Reconstruct the durable state left by the old delivery guard: a saved
    // terminal projection, but no delivery checkpoint/ack for that frame. Read
    // the actual authenticated frame through the provider HTTP adapter.
    let mut lease = fixture
        .journal
        .claim(&fixture.plan, &fixture.plan.targets[0], "old-worker")
        .await
        .unwrap();
    let port = fixture.port(&http);
    let mut projection = fixture
        .journal
        .load_projection(&lease)
        .await
        .unwrap()
        .unwrap();
    let mut attached = port
        .reattach(
            &work.identity,
            &work.target_id,
            &lease,
            projection.native_agent_id.as_deref(),
        )
        .await
        .unwrap();
    let frame = attached.stream.events.recv().await.unwrap().unwrap();
    assert_eq!(frame.sequence_number, 4);
    assert!(storage::bytes(&frame.raw).unwrap().len() > 256 * 1024);
    let body = fixture.journal.body(&lease).await.unwrap();
    projection.project(&frame, &body, false, "failed").unwrap();
    fixture
        .journal
        .save_projection(&mut lease, &projection)
        .await
        .unwrap();
    assert!(
        storage::bytes(&projection).unwrap().len() < 4096,
        "raw terminal tool bodies must not be persisted"
    );
    fixture.journal.release(&mut lease).await.unwrap();
    drop(attached);
    drop(port);

    fixture.records.memory.clock.store(
        fixture.plan.targets[0].launch.event.expires_at + 60,
        Ordering::SeqCst,
    );
    let retained = fixture
        .records
        .store()
        .recover_all_page(None, 32)
        .await
        .unwrap();
    assert!(retained.issues.is_empty());
    assert!(retained
        .plans
        .iter()
        .any(|plan| plan.admission_id == fixture.plan.admission_id));
    // Fresh journal, activity store and executor: no process-local state is
    // needed to settle an already-produced Activity after deployment.
    fixture.activity = Arc::new(ActivityStore::new(
        fixture.records.records.clone(),
        Arc::new(fixture::keys()),
    ));
    let mut journal = Journal::new(fixture.records.records.clone(), Arc::new(fixture::keys()))
        .with_activity(fixture.activity.clone());
    let clock = fixture.records.memory.clock.clone();
    journal.clock = Arc::new(move || clock.load(Ordering::SeqCst));
    fixture.journal = journal;
    let (resumed, _engine, work) = fixture.executor(&http);
    let before = http.seen().len();

    // Even ignored tool output participates in the full raw-frame digest.
    http.memory
        .lock()
        .unwrap()
        .config
        .replay_frames
        .as_mut()
        .unwrap()[4]["response"]["output"][0]["output"] = "y".repeat(265_883).into();
    assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
    assert!(matches!(
        fixture.stored().await.phase,
        Phase::Accepted {
            durable_cursor: Some(3),
            ..
        }
    ));
    assert!(fixture.consumer().await.completed_at.is_none());
    http.memory.lock().unwrap().config.replay_frames = Some(frames());
    assert!(matches!(resumed.execute(&work).await, Disposition::Settled));
    assert!(http.seen()[before..]
        .iter()
        .all(|request| request.method == "GET"));
    assert!(http
        .seen()
        .last()
        .unwrap()
        .uri
        .ends_with("/events?starting_after=3"));
    completed_once(&fixture, &http, &resumed, &work).await;
}

#[tokio::test]
async fn terminal_above_sse_limit_is_rejected_without_ack_or_resubmission() {
    let fixture = Fixture::new().await;
    let http = provider().await;
    let (mut first, _engine, work) = fixture.executor(&http);
    first.frame_limit = 4;
    assert!(matches!(first.execute(&work).await, Disposition::Retry(_)));
    http.memory
        .lock()
        .unwrap()
        .config
        .replay_frames
        .as_mut()
        .unwrap()[4]["response"]["output"][0]["output"] = "x".repeat(1024 * 1024).into();
    let (resumed, _engine, work) = fixture.executor(&http);
    assert!(matches!(
        resumed.execute(&work).await,
        Disposition::Retry(_)
    ));
    assert!(matches!(
        fixture.stored().await.phase,
        Phase::Accepted {
            durable_cursor: Some(3),
            ..
        }
    ));
    assert_eq!(fixture.consumer().await.state, ConsumerState::Produced);
    assert!(fixture.consumer().await.completed_at.is_none());
    assert_eq!(
        http.seen()
            .iter()
            .filter(|request| request.method == "POST")
            .count(),
        1
    );

    let mut lease = fixture
        .journal
        .claim(&fixture.plan, &fixture.plan.targets[0], "inspect-limit")
        .await
        .unwrap();
    let mut attached = fixture
        .port(&http)
        .reattach(&work.identity, &work.target_id, &lease, Some("agt_native"))
        .await
        .unwrap();
    let error = attached.stream.events.recv().await.unwrap().unwrap_err();
    assert!(
        matches!(error, crate::providers::ProviderError::Sse(ref detail) if detail.contains("1 MiB")),
        "{error}"
    );
    fixture.journal.release(&mut lease).await.unwrap();
}
