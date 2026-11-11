//! Retry already-admitted capacity work without deriving authority from discovery.
use std::{collections::VecDeque, sync::Arc, time::SystemTime};

use futures_util::{stream, StreamExt};
use layer2_core::sandbox::agent_capacity_recovery::{
    ports::{CommitOutcome, RecoveryError},
    state::{RecoveryPhase, RecoveryRequest},
};
use tokio::sync::Mutex;

use super::agent_worker::AgentWorker;
use crate::store::agent_capacity_recovery::{
    RecoveryCursor, RecoveryPage, RecoveryService, RecoveryStream,
};

const PAGE_SIZE: usize = 32;
const CONCURRENCY: usize = 2;

#[async_trait::async_trait]
trait AcceptedRecovery: Send + Sync {
    async fn installed(&self) -> Result<bool, RecoveryError>;
    async fn discover(
        &self,
        stream: RecoveryStream,
        after: Option<&RecoveryCursor>,
        now: &str,
    ) -> Result<RecoveryPage, RecoveryError>;
    async fn run(&self, request: &RecoveryRequest) -> Result<bool, RecoveryError>;
}

struct Production {
    store: Arc<crate::store::MongoSandboxStore>,
    service: Arc<RecoveryService>,
    worker: Arc<AgentWorker>,
}

#[async_trait::async_trait]
impl AcceptedRecovery for Production {
    async fn installed(&self) -> Result<bool, RecoveryError> {
        self.store.capacity_recovery_work_exists().await
    }
    async fn discover(
        &self,
        stream: RecoveryStream,
        after: Option<&RecoveryCursor>,
        now: &str,
    ) -> Result<RecoveryPage, RecoveryError> {
        self.service.discover(stream, after, now).await
    }

    async fn run(&self, request: &RecoveryRequest) -> Result<bool, RecoveryError> {
        Ok(matches!(
            self.worker
                .run_accepted_capacity_recovery(&self.service, request)
                .await?,
            CommitOutcome::Recorded(work) if work.phase == RecoveryPhase::Completed
        ))
    }
}

#[derive(Default)]
struct DiscoveryState {
    installed: bool,
    cursors: [Option<RecoveryCursor>; 2],
    lane: usize,
    queued: VecDeque<RecoveryRequest>,
}

pub(super) struct CapacityRecoverySweep {
    work: Arc<dyn AcceptedRecovery>,
    state: Mutex<DiscoveryState>,
}

impl CapacityRecoverySweep {
    pub(super) fn new(
        store: Arc<crate::store::MongoSandboxStore>,
        service: Arc<RecoveryService>,
        worker: Arc<AgentWorker>,
    ) -> Arc<Self> {
        Arc::new(Self {
            work: Arc::new(Production {
                store,
                service,
                worker,
            }),
            state: Mutex::new(DiscoveryState::default()),
        })
    }

    /// Local queue/cursors are disposable hints. Restart begins both durable
    /// sweeps again; only the exact accepted publication can grant a claim.
    pub(super) async fn run_due(&self, now: SystemTime) -> Result<usize, RecoveryError> {
        // Also enforce the global bound if a caller overlaps scheduler sweeps.
        let Ok(mut state) = self.state.try_lock() else {
            return Ok(0);
        };
        // Before the explicit additive migration, do no hinted query and no
        // startup DDL. A positive existence hint grants no execution authority;
        // all record/schema/receipt checks remain in discovery and claim.
        if !state.installed {
            state.installed = self.work.installed().await?;
            if !state.installed {
                return Ok(0);
            }
        }
        if state.queued.is_empty() {
            let lane = state.lane;
            state.lane = 1 - lane;
            let stream = [RecoveryStream::Admitted, RecoveryStream::ExpiredLease][lane];
            let page = self
                .work
                .discover(
                    stream,
                    state.cursors[lane].as_ref(),
                    &layer2_core::rfc3339_at(now),
                )
                .await?;
            if page.requests.len() > PAGE_SIZE {
                return Err(RecoveryError::InvalidEvidence);
            }
            for request in &page.requests {
                request.validate()?;
            }
            state.cursors[lane] = page.next;
            state.queued = page.requests.into();
        }
        // At most two claims per scheduler turn; one slow Agent cannot create
        // unbounded tasks, consume a whole page of leases, or trigger replay.
        let count = CONCURRENCY.min(state.queued.len());
        let batch: Vec<_> = state.queued.drain(..count).collect();
        let work = Arc::clone(&self.work);
        let mut attempts = stream::iter(batch)
            .map(move |request| {
                let work = Arc::clone(&work);
                async move { work.run(&request).await }
            })
            .buffer_unordered(CONCURRENCY);
        let mut completed = 0;
        let mut refused = 0;
        while let Some(result) = attempts.next().await {
            match result {
                Ok(true) => completed += 1,
                Ok(false) => {}
                Err(_) => refused += 1,
            }
        }
        if count > 0 {
            tracing::debug!(
                attempted = count,
                completed,
                refused,
                "Accepted capacity recovery sweep completed"
            );
        }
        Ok(completed)
    }
}

#[cfg(test)]
#[path = "capacity_scheduler_tests.rs"]
mod tests;
