//! Prepare independent identity and capacity while preserving the sealed activity.
use super::{refusal, ConsumerLaunch, FanoutLauncher};
use crate::ingress::launch::{LaunchOutcome, LaunchRefusal, LaunchRequest, TurnLauncher};
use cmaeg_core::{
    activity::ConsumerTarget,
    ids::{ConversationId, IdempotencyKey},
    problem::ProblemCode,
    seal::{EventRef, EventRefPayload, ReplyAddress, SealError},
};
use std::{
    sync::Mutex,
    time::{Duration, Instant},
};
use tokio::sync::OwnedSemaphorePermit;

pub(super) struct Child {
    target: ConsumerTarget,
    payload: EventRefPayload,
    request: Result<LaunchRequest, Box<LaunchRefusal>>,
    received_at: Instant,
}
pub(super) struct CompletedChild {
    pub launch: ConsumerLaunch,
    pub payload: EventRefPayload,
    pub elapsed: Duration,
}
impl Child {
    pub async fn launch(
        self,
        inner: &dyn TurnLauncher,
        hold: Option<OwnedSemaphorePermit>,
    ) -> CompletedChild {
        let outcome = match self.request {
            Err(failure) => {
                drop(hold);
                LaunchOutcome::Refused(*failure)
            }
            Ok(request) => match hold {
                Some(hold) => inner.launch_holding(request, hold).await,
                None => inner.launch(request).await,
            },
        };
        completed(
            self.target,
            self.payload,
            outcome,
            self.received_at.elapsed(),
        )
    }
    /// Preserve every prepared identity, but release leases before reporting
    /// that the finalizer could not be admitted. No child may launch here.
    pub fn refuse(self, refusal: LaunchRefusal) -> CompletedChild {
        drop(self.request);
        completed(
            self.target,
            self.payload,
            LaunchOutcome::Refused(refusal),
            self.received_at.elapsed(),
        )
    }
}
fn completed(
    target: ConsumerTarget,
    payload: EventRefPayload,
    mut outcome: LaunchOutcome,
    elapsed: Duration,
) -> CompletedChild {
    if let LaunchOutcome::Refused(ref mut refusal) = outcome {
        refusal.provider_code = crate::delivery::provider_code(refusal.provider_code.take());
    }
    CompletedChild {
        launch: ConsumerLaunch {
            id: target.id,
            name: target.name.to_string(),
            conversation_id: payload.conversation_id.clone(),
            outcome,
        },
        payload,
        elapsed,
    }
}

pub(super) fn prepare(
    launcher: &FanoutLauncher,
    request: &LaunchRequest,
    base: &EventRefPayload,
    target: &ConsumerTarget,
    index: usize,
) -> Child {
    let consumer = (!target.id.is_default()).then(|| target.id.clone());
    let conversation_id = if target.id.is_default() {
        request.conversation_id.clone()
    } else {
        ConversationId::derive_for_consumer(
            &launcher.keys,
            &request.channel.id,
            request.channel.definition.partition.version,
            &target.id,
            &base.partition_key,
        )
    };
    let mut payload = base.clone();
    payload.consumer = consumer.clone();
    payload.conversation_id = conversation_id.clone();
    if index != 0 && !request.sync_fanout && matches!(payload.reply, ReplyAddress::Sync { .. }) {
        payload.reply = ReplyAddress::None;
    }
    let build = || {
        if request
            .channel
            .consumer(Some(&target.id))
            .is_none_or(|c| c.pending)
        {
            return Err(Box::new(refusal(
                ProblemCode::ProviderUnavailable,
                503,
                "consumer_removed",
            )));
        }
        let lease = launcher
            .capacity
            .try_lease(
                &request.channel.pointer.record_id,
                &request.channel.definition.owner.subject,
            )
            .map_err(|scope| Box::new(refusal(ProblemCode::ProviderBusy, 503, scope.code())))?;
        let event_ref = EventRef::seal(&payload, &launcher.keys).map_err(|error| {
            Box::new(match error {
                SealError::TooLarge => {
                    refusal(ProblemCode::PayloadTooLarge, 413, "event_ref_too_large")
                }
                _ => refusal(ProblemCode::Internal, 500, "event_ref_unavailable"),
            })
        })?;
        Ok(LaunchRequest {
            channel: request.channel.clone(),
            activity: request.activity.clone(),
            conversation_id,
            event_ref,
            idempotency_key: if target.id.is_default() {
                request.idempotency_key.clone()
            } else {
                IdempotencyKey::for_consumer(&request.activity.event_id, &target.id)
            },
            received_at: request.received_at,
            trace: request.trace.clone(),
            timing: if index == 0 {
                request.timing.clone()
            } else {
                None
            },
            consumer,
            sync_fanout: request.sync_fanout,
            lease: Mutex::new(Some(lease)),
        })
    };
    let child = build();
    Child {
        target: target.clone(),
        payload,
        request: child,
        received_at: request.received_at,
    }
}
