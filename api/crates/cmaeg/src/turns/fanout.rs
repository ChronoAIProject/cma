//! One capped provider turn per target, sharing the existing delivery engine.
mod child;
mod finalizer;

use super::{
    capacity::RoutedCapacity,
    observer::{ActivityObserver, OutcomeSummary},
    TurnTracker,
};
use crate::{
    delivery::router::DeliverySink,
    ingress::launch::{LaunchOutcome, LaunchRefusal, LaunchRequest, TurnLauncher},
};
use cmaeg_core::{
    channel::MAX_TARGETS,
    ids::{ConsumerId, ConversationId},
    problem::{Problem, ProblemCode},
    seal::{MasterKeyRing, OpenError},
};
use futures_util::future::{join_all, BoxFuture};
use std::{sync::Arc, time::Duration};
use tokio::sync::{oneshot, OwnedSemaphorePermit};
use tracing::Instrument;

pub struct FanoutLauncher {
    inner: Arc<dyn TurnLauncher>,
    keys: Arc<MasterKeyRing>,
    delivery: Arc<dyn DeliverySink>,
    capacity: Arc<RoutedCapacity>,
    tracker: Arc<TurnTracker>,
    turn_timeout: Duration,
    observer: Arc<dyn ActivityObserver>,
}
#[derive(Debug, Clone)]
pub struct ConsumerLaunch {
    pub id: ConsumerId,
    pub name: String,
    pub conversation_id: ConversationId,
    pub outcome: LaunchOutcome,
}
#[derive(Debug, Clone)]
pub struct FanoutOutcome {
    pub combined: LaunchOutcome,
    pub primary_conversation: ConversationId,
    pub consumers: Vec<ConsumerLaunch>,
}

impl FanoutLauncher {
    pub fn new(
        inner: Arc<dyn TurnLauncher>,
        keys: Arc<MasterKeyRing>,
        delivery: Arc<dyn DeliverySink>,
        capacity: Arc<RoutedCapacity>,
        tracker: Arc<TurnTracker>,
        turn_timeout: Duration,
        observer: Arc<dyn ActivityObserver>,
    ) -> Self {
        Self {
            inner,
            keys,
            delivery,
            capacity,
            tracker,
            turn_timeout,
            observer,
        }
    }

    async fn routed(
        &self,
        request: LaunchRequest,
        mut hold: Option<OwnedSemaphorePermit>,
    ) -> FanoutOutcome {
        let Some(routing) = &request.activity.routing else {
            return single(&*self.inner, request, hold).await;
        };
        if !(1..=usize::from(MAX_TARGETS)).contains(&routing.targets.len()) {
            return self.refuse(
                &request,
                refusal(ProblemCode::InvalidRequest, 400, "invalid_targets"),
            );
        }
        metrics::histogram!(crate::telemetry::names::FANOUT_WIDTH)
            .record(routing.targets.len() as f64);
        let now = time::OffsetDateTime::now_utc().unix_timestamp();
        let payload = match request.event_ref.open(&self.keys, now) {
            Ok(opened) => opened.payload,
            Err(OpenError::Expired) => {
                return self.refuse(
                    &request,
                    refusal(ProblemCode::EventRefExpired, 410, "event_ref_expired"),
                )
            }
            Err(_) => {
                return self.refuse(
                    &request,
                    refusal(ProblemCode::EventRefInvalid, 400, "event_ref_invalid"),
                )
            }
        };
        let children = routing
            .targets
            .iter()
            .enumerate()
            .map(|(i, target)| child::prepare(self, &request, &payload, target, i))
            .collect::<Vec<_>>();
        let finalizer = if children.len() >= 2 {
            let (tx, rx) = oneshot::channel();
            let delivery = self.delivery.clone();
            let channel_hash = crate::telemetry::channel_hash(request.channel.id.as_str());
            let deadline = request.received_at + self.turn_timeout;
            if let Err(reason) = self.tracker.spawn(
                &request,
                finalizer::run(rx, delivery, deadline, channel_hash),
            ) {
                let results = children
                    .into_iter()
                    .map(|child| child.refuse(LaunchRefusal::tracker(reason)))
                    .collect::<Vec<_>>();
                let combined = combine(&results);
                return self.finish(&request, results, combined);
            }
            Some(tx)
        } else {
            None
        };
        let results = join_all(children.into_iter().enumerate().map(|(i, child)| {
            // A refused primary cannot transfer its permit to a sibling.
            let held = if i == 0 { hold.take() } else { None };
            async move { child.launch(&*self.inner, held).await }
        }))
        .await;
        let combined = combine(&results);
        let admitted = !matches!(combined, LaunchOutcome::Refused(_));
        // No await may separate completed acceptance from this hand-off: even
        // if the handler is then cancelled, the tracked finalizer owns failures.
        if let Some(tx) = finalizer {
            let failures = if admitted {
                results
                    .iter()
                    .filter_map(|child| match &child.launch.outcome {
                        LaunchOutcome::Refused(failure) => {
                            Some(crate::delivery::intents::DeliveryCommand::ConsumerFailed {
                                channel: request.channel.clone(),
                                payload: child.payload.clone(),
                                consumer: child.launch.id.clone(),
                                code: failure.code,
                                provider_code: failure.provider_code.clone(),
                            })
                        }
                        _ => None,
                    })
                    .collect()
            } else {
                Vec::new()
            };
            if let Err(failures) = tx.send(failures) {
                finalizer::undelivered(
                    &failures,
                    "closed",
                    &crate::telemetry::channel_hash(request.channel.id.as_str()),
                );
            }
        }
        self.finish(&request, results, combined)
    }

    fn finish(
        &self,
        request: &LaunchRequest,
        results: Vec<child::CompletedChild>,
        combined: LaunchOutcome,
    ) -> FanoutOutcome {
        let primary_conversation = results
            .iter()
            .find(|child| !matches!(child.launch.outcome, LaunchOutcome::Refused(_)))
            .unwrap_or(&results[0])
            .launch
            .conversation_id
            .clone();
        for child in &results {
            self.observe(request, &child.launch, child.elapsed);
        }
        let consumers = results.into_iter().map(|child| child.launch).collect();
        FanoutOutcome {
            combined,
            primary_conversation,
            consumers,
        }
    }

    fn refuse(&self, request: &LaunchRequest, failure: LaunchRefusal) -> FanoutOutcome {
        let outcome = LaunchOutcome::Refused(failure);
        let consumers = request
            .activity
            .routing
            .iter()
            .flat_map(|r| &r.targets)
            .take(usize::from(MAX_TARGETS))
            .map(|t| ConsumerLaunch {
                id: t.id.clone(),
                name: t.name.to_string(),
                conversation_id: request.conversation_id.clone(),
                outcome: outcome.clone(),
            })
            .collect::<Vec<_>>();
        for child in &consumers {
            self.observe(request, child, request.received_at.elapsed());
        }
        FanoutOutcome {
            combined: outcome,
            primary_conversation: request.conversation_id.clone(),
            consumers,
        }
    }
    fn observe(&self, request: &LaunchRequest, child: &ConsumerLaunch, elapsed: Duration) {
        let outcome = match &child.outcome {
            LaunchOutcome::Accepted { .. } => "accepted",
            LaunchOutcome::Deferred => "deferred",
            LaunchOutcome::Refused(failure) => {
                tracing::warn!(channel_hash = %crate::telemetry::channel_hash(request.channel.id.as_str()),
                        consumer = %child.id, code = failure.code, provider_code = failure.provider_code.as_deref(),
                        "routed consumer refused before acceptance");
                match failure.code {
                    "provider_rejected"
                    | "event_ref_too_large"
                    | "channel_capacity"
                    | "owner_capacity" => failure.code,
                    _ => "refused",
                }
            }
        };
        let provider = request
            .channel
            .consumer(Some(&child.id))
            .map_or("unknown", |c| c.provider.slug.as_str());
        metrics::counter!(crate::telemetry::names::FANOUT_TURNS,
                "provider" => provider.to_owned(), "outcome" => outcome)
        .increment(1);
        self.observer.outcome(
            &request.channel.pointer.record_id,
            &OutcomeSummary::of(
                request.activity.event_id.as_str(),
                child.id.clone(),
                &child.outcome,
                elapsed,
            ),
        );
    }
}

impl TurnLauncher for FanoutLauncher {
    fn launch(&self, request: LaunchRequest) -> BoxFuture<'_, LaunchOutcome> {
        self.inner.launch(request)
    }
    fn launch_holding(
        &self,
        request: LaunchRequest,
        hold: OwnedSemaphorePermit,
    ) -> BoxFuture<'_, LaunchOutcome> {
        self.inner.launch_holding(request, hold)
    }
    fn launch_routed(
        &self,
        request: LaunchRequest,
        hold: Option<OwnedSemaphorePermit>,
    ) -> BoxFuture<'_, FanoutOutcome> {
        let targets = request
            .activity
            .routing
            .iter()
            .flat_map(|routing| &routing.targets)
            .take(usize::from(MAX_TARGETS))
            .map(|target| target.id.as_str())
            .collect::<Vec<_>>();
        let span = tracing::info_span!("cmaeg.fanout", targets = ?targets);
        Box::pin(self.routed(request, hold).instrument(span))
    }
}

pub(crate) async fn single<T: TurnLauncher + ?Sized>(
    inner: &T,
    request: LaunchRequest,
    hold: Option<OwnedSemaphorePermit>,
) -> FanoutOutcome {
    let id = request
        .consumer
        .clone()
        .unwrap_or_else(ConsumerId::default_consumer);
    let name = request
        .channel
        .consumer(Some(&id))
        .map_or_else(|| id.to_string(), |c| c.name.to_owned());
    let conversation_id = request.conversation_id.clone();
    let outcome = match hold {
        Some(hold) => inner.launch_holding(request, hold).await,
        None => inner.launch(request).await,
    };
    FanoutOutcome {
        combined: outcome.clone(),
        primary_conversation: conversation_id.clone(),
        consumers: vec![ConsumerLaunch {
            id,
            name,
            conversation_id,
            outcome,
        }],
    }
}

fn combine(results: &[child::CompletedChild]) -> LaunchOutcome {
    results
        .iter()
        .find_map(|child| match &child.launch.outcome {
            accepted @ LaunchOutcome::Accepted { .. } => Some(accepted.clone()),
            _ => None,
        })
        .unwrap_or_else(|| {
            if results
                .iter()
                .any(|c| matches!(c.launch.outcome, LaunchOutcome::Deferred))
            {
                LaunchOutcome::Deferred
            } else {
                results[0].launch.outcome.clone()
            }
        })
}
fn refusal(problem: ProblemCode, status: u16, code: &'static str) -> LaunchRefusal {
    LaunchRefusal::new(Problem::new(problem, status, code), code)
}
