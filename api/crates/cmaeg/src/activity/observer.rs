use super::{ActivityStore, Change};
use crate::{
    delivery::intents::DeliveryOutcome,
    ingress::launch::LaunchRequest,
    turns::observer::{
        ActivityObserver, Causation, DeliveryStep, Destination, LaunchClass, ProducerFacts,
    },
};
use cmaeg_core::{
    activity::Activity,
    ids::{ConsumerId, EventId, RecordId},
    routing::Trace,
};
use futures_util::future::BoxFuture;
use std::sync::Arc;

/// Add durable public activity without changing the retained console observer.
pub struct ObservedActivity {
    store: Arc<ActivityStore>,
    console: Arc<dyn ActivityObserver>,
}
impl ObservedActivity {
    pub fn new(store: Arc<ActivityStore>, console: Arc<dyn ActivityObserver>) -> Self {
        Self { store, console }
    }
}
impl ActivityObserver for ObservedActivity {
    fn receive<'a>(&'a self, request: &'a LaunchRequest) -> BoxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.store.received(request),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ => {
                    tracing::warn!("channel activity admission persistence unavailable");
                    Err(())
                }
            }
        })
    }
    fn receive_at<'a>(
        &'a self,
        owner: &'a str,
        record: &'a RecordId,
        activity: &'a Activity,
        received_at: i64,
    ) -> BoxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            let received =
                time::OffsetDateTime::from_unix_timestamp(received_at).map_err(|_| ())?;
            match tokio::time::timeout(
                std::time::Duration::from_secs(5),
                self.store
                    .received_at(owner, record.as_str(), activity, received),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ => {
                    tracing::warn!("channel activity intake persistence unavailable");
                    Err(())
                }
            }
        })
    }
    fn checkpoint<'a>(
        &'a self,
        record: &'a RecordId,
        event: &'a EventId,
        change: Change,
    ) -> BoxFuture<'a, ()> {
        Box::pin(async move {
            self.store
                .observe(record.as_str(), event.as_str(), &change)
                .await;
        })
    }
    fn admitted(&self, record: &RecordId, activity: &Activity) {
        self.console.admitted(record, activity);
    }
    fn platform_receipt<'a>(
        &'a self,
        record: &'a RecordId,
        event: &'a EventId,
        change: Change,
    ) -> BoxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.store.change(record.as_str(), event.as_str(), &change),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ => Err(()),
            }
        })
    }
    fn platform_receipts<'a>(
        &'a self,
        owner: &'a str,
        record: &'a RecordId,
        changes: Vec<(EventId, Change)>,
    ) -> BoxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            match tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.store.change_batch(owner, record.as_str(), &changes),
            )
            .await
            {
                Ok(Ok(())) => Ok(()),
                _ => Err(()),
            }
        })
    }
    fn routed(&self, record: &RecordId, activity: &Activity, trace: &Trace) {
        self.console.routed(record, activity, trace);
    }
    fn route_refused(&self, record: &RecordId, activity: &Activity, reason: &str, trace: &Trace) {
        self.console.route_refused(record, activity, reason, trace);
    }
    fn refused(
        &self,
        record: &RecordId,
        source: &'static str,
        event: Option<&EventId>,
        reason: &str,
    ) {
        self.console.refused(record, source, event, reason);
    }
    fn source_refused<'a>(
        &'a self,
        owner: &'a str,
        record: &'a RecordId,
        receipt: super::ChannelActivity,
        received: time::OffsetDateTime,
    ) -> BoxFuture<'a, Result<(), ()>> {
        Box::pin(async move {
            let write =
                self.store
                    .persist_receipt(owner, record.as_str(), receipt.clone(), received);
            let _ = tokio::time::timeout(std::time::Duration::from_secs(3), write).await;
            // An acknowledgement can be lost after the commit. Verify the exact
            // retained receipt, including its outcome, before confirming refusal.
            let page = tokio::time::timeout(
                std::time::Duration::from_secs(3),
                self.store.latest(owner, record.as_str()),
            )
            .await
            .map_err(|_| ())?
            .map_err(|_| ())?;
            page.items
                .iter()
                .any(|row| {
                    row.event_id == receipt.event_id
                        && row.source_refusal == receipt.source_refusal
                        && row.consumers.is_empty()
                })
                .then_some(())
                .ok_or(())
        })
    }
    fn launched(
        &self,
        record: &RecordId,
        event: &EventId,
        consumer: &ConsumerId,
        outcome: LaunchClass,
        accepted: std::time::Duration,
    ) {
        self.console
            .launched(record, event, consumer, outcome, accepted);
    }
    fn delivered(
        &self,
        record: &RecordId,
        event: &EventId,
        consumer: &ConsumerId,
        step: DeliveryStep,
        outcome: &DeliveryOutcome,
        destination: Destination,
    ) {
        self.console
            .delivered(record, event, consumer, step, outcome, destination);
    }
    fn published(&self, record: &RecordId, event: &EventId, causation: Causation<'_>) {
        self.console.published(record, event, causation);
    }
    fn produced(&self, record: &RecordId, producer: ProducerFacts<'_>) {
        self.console.produced(record, producer);
    }
    fn unauthenticated(&self, source: &'static str) {
        self.console.unauthenticated(source);
    }
}
