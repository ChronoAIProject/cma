//! Expiry is terminal only when the durable journal proves no Responses start.
use super::{
    dispatcher::admission_error, journal, model::delivery_event, ExecutionError, Journal, Lease,
    Phase, RoutedDelivery, TargetState,
};
use crate::{
    activity::{Change, ConsumerState},
    ingress::routing_store::EventIdentity,
};
use cmaeg_core::problem::{Problem, ProblemCode};

pub(super) const EXPIRED_BEFORE_START: &str = "event_expired_before_start";

fn unstarted(state: &TargetState) -> bool {
    state.delivery.is_none() && state.projection.is_none()
}

impl Journal {
    pub(super) async fn expire_pending(
        &self,
        lease: &mut Lease,
    ) -> Result<Option<Phase>, ExecutionError> {
        // Expired also describes ancestors, read/replay and delivery authority.
        // Only this target's deadline and retained start evidence decide here.
        if lease.state.event_expires_at > (self.clock)()
            || !matches!(lease.state.phase, Phase::Pending | Phase::Prepared)
            || !unstarted(&lease.state)
        {
            return Ok(None);
        }
        let phase = Phase::Refused {
            code: EXPIRED_BEFORE_START.into(),
        };
        // Prepared proves no start, including a precise non-admission refusal.
        // Keep binding_preparation unchanged: Pending can retain an ambiguous
        // reservation or a confirmed hierarchy receipt. Neither is cancelled
        // or retried by terminalizing the never-started Responses target.
        self.transition(lease, phase.clone()).await?;
        Ok(Some(phase))
    }
}

impl RoutedDelivery {
    /// Repair reporting from durable terminal evidence on every retained replay.
    /// This grants no provider, webhook or platform delivery authority.
    pub(super) async fn report_expired_before_start(
        &self,
        identity: &EventIdentity,
        target_id: &str,
    ) -> Result<(), ExecutionError> {
        let plan = self
            .plans
            .load(identity)
            .await
            .map_err(admission_error)?
            .ok_or(ExecutionError::Invalid)?;
        let target = plan
            .targets
            .iter()
            .find(|target| target.id == target_id)
            .ok_or(ExecutionError::Invalid)?;
        let (state, _) = self
            .journal
            .read::<TargetState>(target_id)
            .await?
            .ok_or(ExecutionError::Unavailable)?;
        journal::validate(&state, &plan, target)?;
        let now = (self.journal.clock)();
        if state.retained_until <= now {
            return Err(ExecutionError::Expired);
        }
        if !matches!(&state.phase, Phase::Refused { code } if code == EXPIRED_BEFORE_START)
            || state.event_expires_at > now
            || !unstarted(&state)
        {
            return Ok(());
        }
        self.journal
            .observe(
                identity,
                Change::state(target.launch.consumer.as_str(), ConsumerState::Failed, None),
            )
            .await;
        if let Some(custom) = &self.coordinator.custom {
            // Only the original primary Sync waiter can receive this content-
            // free control frame. Atomic terminal enqueue removes that waiter.
            custom.refuse_local(
                &delivery_event(&plan, target),
                Problem::new(ProblemCode::EventRefExpired, 410, EXPIRED_BEFORE_START),
            );
        }
        Ok(())
    }
}
