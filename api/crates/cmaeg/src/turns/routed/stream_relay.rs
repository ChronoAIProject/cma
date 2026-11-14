//! One bounded, authenticated hop to the original live Custom stream. No
//! provider operation is retried, and an uncertain hop remains Lost evidence.
#[cfg(test)]
mod mongo_tests;
pub(super) mod monitor;
mod receive;
#[cfg(test)]
mod tests;
pub(super) use receive::receive;

use super::{
    model::Ownership,
    storage,
    tool_relay::codec::{self, Direction, Envelope},
    DeliveryCheckpoint, ExecutionError, Lease, RoutedDelivery,
};
use crate::{
    delivery::local_stream::{CustomStreamEffect, LocalStreamOutcome},
    ingress::routing_store::{AdmittedPlan, EventIdentity, StoredTarget},
    replicas::partition::PartitionFence,
};
use cmaeg_core::seal::{EventRefPayload, ReplyAddress};
use serde::{Deserialize, Serialize};
use std::time::Duration;

pub(super) const PATH: &str = "/internal/replicas/custom-stream";
const CONTEXT: &str = "POST /internal/replicas/custom-stream";
const WINDOW: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    identity: EventIdentity,
    target: String,
    ownership: Ownership,
    partition: PartitionFence,
    checkpoint: DeliveryCheckpoint,
    effect: CustomStreamEffect,
    channel_version: u64,
}

pub(super) fn waiter_owner(event: &EventRefPayload) -> Option<&str> {
    let ReplyAddress::Sync { waiter } = &event.reply else {
        return None;
    };
    let (instance, nonce) = waiter.split_once(':')?;
    let id = uuid::Uuid::parse_str(nonce).ok()?;
    (instance.len() == 64
        && instance
            .bytes()
            .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        && id.get_version_num() == 4
        && id.to_string() == nonce)
        .then_some(instance)
}

impl RoutedDelivery {
    pub(crate) fn with_stream_relay(mut self) -> Result<Self, ExecutionError> {
        if self.journal.partitions.is_none() {
            return Err(ExecutionError::Unsupported);
        }
        self.stream_relay = Some(
            reqwest::Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(1))
                .timeout(WINDOW)
                .build()
                .map_err(|_| ExecutionError::Unavailable)?,
        );
        Ok(self)
    }

    pub(super) fn remote_waiter(&self, event: &EventRefPayload) -> bool {
        self.journal.partitions.as_ref().is_some_and(|partitions| {
            waiter_owner(event).is_some_and(|owner| owner != partitions.membership().instance)
        })
    }

    pub(super) async fn forward_stream(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &Lease,
        effect: &CustomStreamEffect,
        channel_version: u64,
    ) -> LocalStreamOutcome {
        let outcome = match tokio::time::timeout(
            WINDOW,
            self.send_stream(plan, target, lease, effect, channel_version),
        )
        .await
        {
            Ok(Ok(outcome)) => outcome,
            // The receiver may have enqueued before its acknowledgement was
            // lost. Do not resend, including after recovery of this intent.
            _ => LocalStreamOutcome::Lost,
        };
        metrics::counter!("cmaeg_replica_stream_relay_total", "outcome" => match outcome {
            LocalStreamOutcome::Buffered => "buffered",
            LocalStreamOutcome::Unavailable => "unavailable",
            _ => "lost",
        })
        .increment(1);
        outcome
    }

    async fn send_stream(
        &self,
        plan: &AdmittedPlan,
        target: &StoredTarget,
        lease: &Lease,
        effect: &CustomStreamEffect,
        channel_version: u64,
    ) -> Result<LocalStreamOutcome, ExecutionError> {
        let partitions = self
            .journal
            .partitions
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?;
        let event = super::model::delivery_event(plan, target);
        let owner = waiter_owner(&event).ok_or(ExecutionError::Invalid)?;
        let origin = match partitions.membership().origin(owner) {
            Ok(origin) => origin,
            Err(_) => return Ok(LocalStreamOutcome::Unavailable),
        };
        let origin = crate::replicas::config::validate_address(&origin)
            .map_err(|_| ExecutionError::Invalid)?;
        self.journal.guard(lease).await?;
        let request = Request {
            identity: plan.identity.clone(),
            target: target.id.clone(),
            ownership: lease.state.ownership.clone(),
            partition: lease
                .partition
                .as_ref()
                .ok_or(ExecutionError::LostLease)?
                .fence(),
            checkpoint: lease
                .state
                .delivery
                .clone()
                .ok_or(ExecutionError::Invalid)?,
            effect: effect.clone(),
            channel_version,
        };
        let now = (self.journal.clock)();
        let id = uuid::Uuid::new_v4().to_string();
        let envelope = codec::seal(
            &self.journal.keys,
            Direction::StreamRequest,
            &id,
            CONTEXT,
            now.saturating_add(WINDOW.as_secs() as i64),
            now,
            &request,
        )?;
        let bytes = storage::bytes(&envelope)?;
        let request_hash = storage::hash(&bytes);
        let mut response = self
            .stream_relay
            .as_ref()
            .ok_or(ExecutionError::Unsupported)?
            .post(format!("{}{PATH}", origin.trim_end_matches('/')))
            .header(reqwest::header::CONTENT_TYPE, "application/json")
            .body(bytes)
            .send()
            .await
            .map_err(|_| ExecutionError::Uncertain)?;
        if !response.status().is_success()
            || response
                .content_length()
                .is_some_and(|n| n > codec::BODY_LIMIT as u64)
        {
            return Err(ExecutionError::Uncertain);
        }
        let mut bytes = Vec::new();
        while let Some(chunk) = response
            .chunk()
            .await
            .map_err(|_| ExecutionError::Uncertain)?
        {
            if bytes.len().saturating_add(chunk.len()) > codec::BODY_LIMIT {
                return Err(ExecutionError::Uncertain);
            }
            bytes.extend_from_slice(&chunk);
        }
        let envelope: Envelope =
            serde_json::from_slice(&bytes).map_err(|_| ExecutionError::Uncertain)?;
        if envelope.id != id {
            return Err(ExecutionError::Uncertain);
        }
        let outcome: LocalStreamOutcome = codec::open(
            &self.journal.keys,
            Direction::StreamResponse,
            &request_hash,
            (self.journal.clock)(),
            &envelope,
        )?;
        match outcome {
            LocalStreamOutcome::Buffered | LocalStreamOutcome::Unavailable => Ok(outcome),
            _ => Err(ExecutionError::Uncertain),
        }
    }
}
