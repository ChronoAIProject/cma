//! Managed target polling retains command identity across cancellation.
use crate::{
    release_authority::{
        NativePollCommand, NativePollExecution, NativePollLane, NativePollOutcome,
        NativePollReceipt,
    },
    runtime_release::Session,
    warm_pools::PoolTarget,
};
use layer1_public::rest::admin::warm_pools::WarmPoolError;
use std::sync::Arc;
use tokio::sync::Mutex;

pub(super) struct ManagedPolls {
    session: Arc<Session>,
    background: Mutex<PendingPoll>,
    startup_agent: Mutex<PendingPoll>,
    startup_machine: Mutex<PendingPoll>,
}

#[derive(Default)]
struct PendingPoll {
    previous: Option<NativePollReceipt>,
    command: Option<NativePollCommand>,
    retry_noncommitted: bool,
    delivered: Option<DeliveredLease>,
}

struct DeliveredLease {
    target: String,
    revision: i64,
    lease: String,
}

impl DeliveredLease {
    fn matches(&self, target: &PoolTarget) -> bool {
        self.target == target.id
            && self.revision == target.revision
            && target.lease.as_ref() == Some(&self.lease)
    }
}

impl PendingPoll {
    fn acknowledged_retirement(&mut self, prior: &PoolTarget, next: &PoolTarget) {
        if let Some(delivered) = self.delivered.as_mut().filter(|delivered| {
            delivered.matches(prior)
                && prior.revision.checked_add(1) == Some(next.revision)
                && next.id == prior.id
                && next.lease == prior.lease
                && next.plan == prior.plan
                && next.binding == prior.binding
                && !prior.retiring
                && next.retiring
                && !next.retired
                && next.target_replicas == 0
        }) {
            // The immutable poll receipt remains in previous. Only the
            // exact acknowledged store transition advances this lease.
            delivered.revision = next.revision;
        }
    }

    fn acknowledged_finish(&mut self, target: &PoolTarget) {
        if self
            .delivered
            .as_ref()
            .is_some_and(|lease| lease.matches(target))
        {
            self.delivered = None;
        }
    }
}

impl ManagedPolls {
    pub(super) fn new(session: Arc<Session>) -> Self {
        Self {
            session,
            background: Mutex::new(PendingPoll::default()),
            startup_agent: Mutex::new(PendingPoll::default()),
            startup_machine: Mutex::new(PendingPoll::default()),
        }
    }

    async fn lane(&self, target: Option<&str>) -> Result<NativePollLane, WarmPoolError> {
        let Some(target) = target else {
            return Ok(NativePollLane::Background);
        };
        let head = self
            .session
            .current()
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        if head.pair.agent.target_id == target {
            Ok(NativePollLane::StartupAgent)
        } else if head
            .pair
            .machine
            .as_ref()
            .is_some_and(|selected| selected.target_id == target)
        {
            Ok(NativePollLane::StartupMachine)
        } else {
            Err(WarmPoolError::Conflict)
        }
    }

    fn state(&self, lane: NativePollLane) -> &Mutex<PendingPoll> {
        match lane {
            NativePollLane::Background => &self.background,
            NativePollLane::StartupAgent => &self.startup_agent,
            NativePollLane::StartupMachine => &self.startup_machine,
        }
    }

    pub(super) async fn claim(
        &self,
        target: Option<&str>,
    ) -> Result<Option<PoolTarget>, WarmPoolError> {
        let lane = self.lane(target).await?;
        poll(self.state(lane), self.session.as_ref(), lane).await
    }

    /// Only the exact acknowledged Mongo finish CAS reaches this method.
    /// It does not settle any provider permit or certify workflow finality.
    pub(super) async fn finished(&self, target: &PoolTarget) {
        for state in [&self.background, &self.startup_agent, &self.startup_machine] {
            state.lock().await.acknowledged_finish(target);
        }
    }

    pub(super) async fn began_retirement(&self, prior: &PoolTarget, next: &PoolTarget) {
        for state in [&self.background, &self.startup_agent, &self.startup_machine] {
            state.lock().await.acknowledged_retirement(prior, next);
        }
    }
}

#[async_trait::async_trait]
trait PollTransactions: Send + Sync {
    async fn prepare(
        &self,
        lane: NativePollLane,
        previous: Option<&NativePollReceipt>,
    ) -> crate::release_authority::Result<NativePollCommand>;
    async fn execute(
        &self,
        command: &NativePollCommand,
    ) -> crate::release_authority::Result<NativePollExecution>;
    async fn reconcile(
        &self,
        command: &NativePollCommand,
    ) -> crate::release_authority::Result<Option<NativePollReceipt>>;
}

#[async_trait::async_trait]
impl PollTransactions for Session {
    async fn prepare(
        &self,
        lane: NativePollLane,
        previous: Option<&NativePollReceipt>,
    ) -> crate::release_authority::Result<NativePollCommand> {
        self.store
            .prepare_native_poll(&self.writer, lane, previous)
            .await
    }
    async fn execute(
        &self,
        command: &NativePollCommand,
    ) -> crate::release_authority::Result<NativePollExecution> {
        self.store.execute_native_poll(command).await
    }
    async fn reconcile(
        &self,
        command: &NativePollCommand,
    ) -> crate::release_authority::Result<Option<NativePollReceipt>> {
        self.store.reconcile_native_poll(command).await
    }
}

async fn poll(
    state: &Mutex<PendingPoll>,
    authority: &dyn PollTransactions,
    lane: NativePollLane,
) -> Result<Option<PoolTarget>, WarmPoolError> {
    let mut pending = state.lock().await;
    if pending.delivered.is_some() {
        return Err(WarmPoolError::Unavailable);
    }
    if let Some(command) = pending
        .command
        .as_ref()
        .filter(|_| !pending.retry_noncommitted)
    {
        let receipt = authority
            .reconcile(command)
            .await
            .map_err(|_| WarmPoolError::Unavailable)?
            .ok_or(WarmPoolError::Unavailable)?;
        return observe(&mut pending, receipt, false);
    }
    if pending.command.is_none() {
        let command = authority
            .prepare(lane, pending.previous.as_ref())
            .await
            .map_err(|_| WarmPoolError::Unavailable)?;
        tracing::debug!(sequence = command.sequence(), "native target poll prepared");
        pending.command = Some(command);
    }
    // Save before the first mutating await. A cancelled task leaves this
    // command in place and its successor can only inspect the same result.
    // Consume the store's noncommit certificate before await. Cancellation
    // or any unclassified error restores reconcile-only handling. Execute at
    // most once per poll call, always retaining the same frozen command.
    pending.retry_noncommitted = false;
    let outcome = authority
        .execute(pending.command.as_ref().expect("saved before dispatch"))
        .await
        .map_err(|_| WarmPoolError::Unavailable)?;
    match outcome {
        NativePollExecution::Fresh(receipt) => observe(&mut pending, receipt, true),
        NativePollExecution::Replay(receipt) => observe(&mut pending, receipt, false),
        NativePollExecution::NotCommitted(error) => {
            pending.retry_noncommitted = true;
            tracing::warn!(error = %error, "native target poll did not commit; same command may retry");
            Err(WarmPoolError::Unavailable)
        }
        NativePollExecution::Uncertain => {
            tracing::warn!("native target poll commit is unresolved; reconciliation required");
            Err(WarmPoolError::Unavailable)
        }
    }
}

fn observe(
    pending: &mut PendingPoll,
    receipt: NativePollReceipt,
    fresh: bool,
) -> Result<Option<PoolTarget>, WarmPoolError> {
    if pending.command.as_ref().is_none_or(|command| {
        command.key() != receipt.key()
            || command.lane() != receipt.lane()
            || command.sequence() != receipt.sequence()
    }) {
        return Err(WarmPoolError::Unavailable);
    }
    let target = match receipt.outcome() {
        NativePollOutcome::Empty => None,
        NativePollOutcome::Claimed(target) if fresh => {
            let target = super::validate((**target).clone())?;
            let lease = target.lease.clone().ok_or(WarmPoolError::Unavailable)?;
            pending.delivered = Some(DeliveredLease {
                target: target.id.clone(),
                revision: target.revision,
                lease,
            });
            Some(target)
        }
        // Receiving the same lease again does not prove whether the previous
        // consumer dispatched its provider operation before losing its result.
        NativePollOutcome::Claimed(_) => {
            tracing::warn!(
                sequence = receipt.sequence(),
                "native target lease replay cannot dispatch provider work"
            );
            return Err(WarmPoolError::Unavailable);
        }
    };
    pending.previous = Some(receipt);
    pending.command = None;
    Ok(target)
}

#[cfg(test)]
#[path = "native_poll_tests.rs"]
mod tests;
