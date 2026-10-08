use super::{routing_fixture, support::Fixture};
use base64::{engine::general_purpose::STANDARD, Engine as _};
use cmaeg::{
    delivery::router::{DeliverySink, ShardedScheduler},
    ingress::launch::{LaunchOutcome, LaunchRefusal, LaunchRequest, TurnLauncher},
    turns::{
        capacity::RoutedCapacity,
        fanout::FanoutLauncher,
        observer::{ActivityObserver, OutcomeSummary},
        outcomes::DeliveryCommand,
        TurnTracker,
    },
    RoutedCapacityConfig,
};
use cmaeg_core::{
    activity::{ActivityRouting, ConsumerTarget, RouteResolution},
    channel::{ChannelDefinition, SourceId},
    ids::ConsumerId,
    problem::{Problem, ProblemCode},
    seal::{EventRef, MasterKeyRing},
};
use futures_util::future::BoxFuture;
use std::{
    collections::BTreeMap,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};
use tokio::sync::{mpsc, Notify, OwnedSemaphorePermit};

pub fn keys() -> Arc<MasterKeyRing> {
    Arc::new(MasterKeyRing::parse(&format!("1:{}", STANDARD.encode([1; 32]))).unwrap())
}
pub fn add_consumers(definition: &mut ChannelDefinition) {
    let routed = routing_fixture::definition(routing_fixture::routed());
    definition.routing = routed.routing;
    definition.secrets.consumer_keys = routed.secrets.consumer_keys;
}
pub fn route(mut request: LaunchRequest, targets: &[&str]) -> LaunchRequest {
    request.channel = super::preparation_support::rebound(&request.channel, add_consumers);
    request.activity.routing = Some(ActivityRouting {
        source_id: SourceId::parse("custom").unwrap(),
        resolution: RouteResolution::EventField,
        rule_id: None,
        targets: targets
            .iter()
            .map(|id| {
                let id = ConsumerId::parse(id).unwrap();
                let name = cmaeg_core::ids::ConsumerName::parse(
                    request.channel.consumer(Some(&id)).unwrap().name,
                )
                .unwrap();
                ConsumerTarget { id, name }
            })
            .collect(),
        addressing_bytes: 0,
        causation: None,
    });
    reseal(&mut request);
    request
}
pub fn reseal(request: &mut LaunchRequest) {
    let mut payload = request
        .event_ref
        .open(&keys(), time::OffsetDateTime::now_utc().unix_timestamp())
        .unwrap()
        .payload;
    payload.activity_sha256 = Some(cmaeg_core::seal::event_ref::activity_digest(
        &request.activity,
    ));
    request.event_ref = EventRef::seal(&payload, &keys()).unwrap();
}
pub fn refused(code: &'static str) -> LaunchOutcome {
    LaunchOutcome::Refused(LaunchRefusal::new(
        Problem::new(ProblemCode::ProviderUnavailable, 503, code),
        code,
    ))
}
#[derive(Default)]
pub struct Recorder {
    pub requests: Mutex<Vec<(LaunchRequest, bool)>>,
    pub outcomes: Mutex<BTreeMap<String, LaunchOutcome>>,
    pub block: Option<Arc<Notify>>,
    pub delays: Mutex<BTreeMap<String, Duration>>,
    pub started: Notify,
}
impl Recorder {
    async fn run(
        &self,
        request: LaunchRequest,
        hold: Option<OwnedSemaphorePermit>,
    ) -> LaunchOutcome {
        let id = request
            .consumer
            .as_ref()
            .map_or("default", ConsumerId::as_str);
        let delay = self
            .delays
            .lock()
            .unwrap()
            .get(id)
            .copied()
            .unwrap_or_default();
        let outcome = self
            .outcomes
            .lock()
            .unwrap()
            .get(id)
            .cloned()
            .unwrap_or_else(|| LaunchOutcome::Accepted {
                response_id: format!("response-{id}"),
            });
        // Like the real execute entry, this fake owns a lease only while running.
        let lease = request.lease.lock().unwrap().take();
        self.requests
            .lock()
            .unwrap()
            .push((request, hold.is_some()));
        self.started.notify_one();
        tokio::time::sleep(delay).await;
        if let Some(release) = &self.block {
            release.notified().await;
        }
        drop(lease);
        drop(hold);
        outcome
    }
}
impl TurnLauncher for Recorder {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(self.run(request, None))
    }
    fn launch_holding(
        &self,
        request: LaunchRequest,
        hold: OwnedSemaphorePermit,
    ) -> BoxFuture<'_, LaunchOutcome> {
        Box::pin(self.run(request, Some(hold)))
    }
}
#[derive(Default)]
pub struct Observer(pub Mutex<Vec<OutcomeSummary>>);
impl ActivityObserver for Observer {
    fn outcome(&self, _: &cmaeg_core::ids::RecordId, summary: &OutcomeSummary) {
        self.0.lock().unwrap().push(summary.clone());
    }
}

pub struct Harness {
    pub base: Fixture,
    pub recorder: Arc<Recorder>,
    pub capacity: Arc<RoutedCapacity>,
    pub delivery: Arc<dyn DeliverySink>,
    pub outcomes: mpsc::Receiver<DeliveryCommand>,
    pub observer: Arc<Observer>,
}
impl Harness {
    pub async fn new() -> Self {
        let base = Fixture::new(Default::default(), Default::default(), 8).await;
        let (sender, outcomes) = mpsc::channel(1);
        Self {
            base,
            recorder: Arc::default(),
            capacity: Arc::new(RoutedCapacity::new(RoutedCapacityConfig::default())),
            delivery: Arc::new(ShardedScheduler::new(sender)),
            outcomes,
            observer: Arc::default(),
        }
    }
    pub fn request(&self, targets: &[&str]) -> LaunchRequest {
        route(self.base.request("fanout-event"), targets)
    }
    pub fn launcher(&self) -> FanoutLauncher {
        self.wrap(self.recorder.clone(), Duration::from_secs(2))
    }
    pub fn wrap(&self, inner: Arc<dyn TurnLauncher>, timeout: Duration) -> FanoutLauncher {
        FanoutLauncher::new(
            inner,
            keys(),
            self.delivery.clone(),
            self.capacity.clone(),
            self.base.tracker.clone(),
            timeout,
            self.observer.clone(),
        )
    }
    pub async fn failure(&mut self) -> DeliveryCommand {
        tokio::time::timeout(Duration::from_secs(1), self.outcomes.recv())
            .await
            .unwrap()
            .unwrap()
    }
}
pub async fn idle(tracker: &TurnTracker) {
    tokio::time::timeout(Duration::from_secs(1), async {
        while !tracker.is_empty() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
}
pub fn elapsed_bound(started: Instant) {
    assert!(started.elapsed() < Duration::from_millis(250));
}
