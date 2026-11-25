//! Finite, recoverable funding. Lifecycle callers only see provider-neutral ports.
use crate::{config::SandboxBillingConfig, store::MongoSandboxStore};
use async_trait::async_trait;
use layer2_core::{
    sandbox::Sandbox,
    sandbox_usage::{Allocation, UsageEvent},
    SandboxId,
};
use serde::{Deserialize, Serialize};
use std::{
    sync::Arc,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

mod admission;
mod evidence;
mod model;
mod mongo;
mod nyxid;
mod runtime_scope;
pub(crate) use runtime_scope::{FundedRuntime, FundingRuntime};
mod recovery;
#[cfg(test)]
mod tests;
pub(crate) use model::*;
use nyxid::NyxIdFundingProvider;
#[cfg(test)]
pub(crate) use tests::pilot::excluded_funding;

const WINDOW_SECONDS: u64 = 300;
const RENEWAL_MARGIN_SECONDS: u64 = 30;
const MAX_PROVIDER_RESPONSE_BYTES: usize = 512 * 1024;

#[async_trait]
pub(crate) trait FundingProvider: Send + Sync {
    async fn quote(&self, request: QuoteRequest) -> Result<FundingQuote, FundingError>;
    async fn reserve(&self, request: ReserveRequest) -> Result<FundingWindow, FundingError>;
    async fn recover(&self, request_id: &str) -> Result<Option<FundingWindow>, FundingError>;
    async fn settle(&self, request: EvidenceRequest) -> Result<(), FundingError>;
}

#[async_trait]
pub(crate) trait FundingStateStore: Send + Sync {
    async fn load(&self, id: &SandboxId) -> Result<Option<SandboxBillingState>, FundingError>;
    async fn save(&self, state: &mut SandboxBillingState) -> Result<(), FundingError>;
    async fn events(&self, id: &SandboxId, after: u64) -> Result<Vec<UsageEvent>, FundingError>;
    async fn checkpoint(&self, id: &SandboxId, now: SystemTime) -> Result<(), FundingError>;
    async fn due(&self, now: u64) -> Result<Vec<SandboxBillingState>, FundingError>;
    async fn sandbox(&self, id: &SandboxId) -> Result<Option<Sandbox>, FundingError>;
}

#[async_trait]
pub(crate) trait FundingControl: Send + Sync {
    /// Confirms an exact incarnation's native stop deadline, retaining its home.
    async fn deadline(&self, runtime: FundedRuntime<'_>) -> Result<(), FundingError>;
    async fn pause(
        &self,
        runtime: FundingRuntime<'_>,
        detail: &str,
        now: SystemTime,
    ) -> Result<(), FundingError>;
}

#[async_trait]
pub(crate) trait SandboxFundingAdapter: Send + Sync {
    /// Returns the confirmed exclusive cutoff for the provider creation/resume.
    async fn ensure_coverage(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<Option<u64>, FundingError>;
    /// Marks compute stopped; evidence is delivered asynchronously after ledger commit.
    async fn settle(&self, sandbox: &Sandbox, now: SystemTime) -> Result<(), FundingError>;
    async fn renew_due(&self, now: SystemTime) -> Result<usize, FundingError>;
}

#[derive(Default)]
pub(crate) struct NoopFundingAdapter;
#[async_trait]
impl SandboxFundingAdapter for NoopFundingAdapter {
    async fn ensure_coverage(
        &self,
        _: &Sandbox,
        _: SystemTime,
    ) -> Result<Option<u64>, FundingError> {
        Ok(None)
    }
    async fn settle(&self, _: &Sandbox, _: SystemTime) -> Result<(), FundingError> {
        Ok(())
    }
    async fn renew_due(&self, _: SystemTime) -> Result<usize, FundingError> {
        Ok(0)
    }
}

pub(crate) struct ProviderFundingAdapter {
    provider: Arc<dyn FundingProvider>,
    state: Arc<dyn FundingStateStore>,
    control: Arc<dyn FundingControl>,
}
impl ProviderFundingAdapter {
    fn new(
        provider: Arc<dyn FundingProvider>,
        state: Arc<dyn FundingStateStore>,
        control: Arc<dyn FundingControl>,
    ) -> Arc<Self> {
        Arc::new(Self {
            provider,
            state,
            control,
        })
    }

    async fn failure(
        &self,
        sandbox: &Sandbox,
        state: &mut SandboxBillingState,
        error: &FundingError,
        now: SystemTime,
    ) -> Result<(), FundingError> {
        if matches!(error, FundingError::Conflict) {
            return Ok(());
        }
        state.attempts = state.attempts.saturating_add(1);
        // Bound retries and stagger replicas using a persisted random operation identity.
        let jitter = state.operation_id.bytes().map(u64::from).sum::<u64>() % 3;
        let delay = match error {
            FundingError::RateLimited(delay) => *delay,
            _ => (1_u64 << state.attempts.min(6)).min(60) + jitter,
        };
        state.retry_at = seconds(now).saturating_add(delay);
        state.last_error = Some(error.to_string());
        self.state.save(state).await?;
        if let Some(runtime) = FundingRuntime::for_state(state, sandbox)? {
            self.control.pause(runtime, &error.to_string(), now).await?;
        }
        Ok(())
    }

    async fn reconcile(
        &self,
        mut state: SandboxBillingState,
        now: SystemTime,
    ) -> Result<(), FundingError> {
        // Excluded generations are admitted only by lifecycle callers, never a sweep.
        if state.unbilled_generation.is_some() {
            return Ok(());
        }
        let id = SandboxId(state.sandbox_id.clone());
        let Some(sandbox) = self.state.sandbox(&id).await? else {
            return self.reconcile_missing(state, now).await;
        };
        runtime_scope::require_identity(&state, &sandbox)?;
        if state.pending.is_some() {
            if let Err(error) = self.recover_pending(&mut state).await {
                self.failure(&sandbox, &mut state, &error, now).await?;
                return Err(error);
            }
        }
        if !state.windows.is_empty() {
            self.state.checkpoint(&id, now).await?;
        }
        // Evidence outages must not discard a confirmed funding receipt.
        if let Err(error) = self.emit_evidence(&mut state, seconds(now)).await {
            if matches!(error, FundingError::Conflict) {
                return Err(error);
            }
            state.last_error = Some(error.to_string());
            self.state.save(&mut state).await?;
        }
        let observed = Allocation::from_sandbox(&sandbox);
        state.retained_disk_gib = observed.disk_gib;
        let replacement_pending = generation(&sandbox) != state.runtime_generation
            || matches!(
                sandbox.status,
                layer2_core::sandbox::SandboxStatus::Reviving
                    | layer2_core::sandbox::SandboxStatus::Rotating
            );
        // Initial provisioning is admitted by its worker, never resurrected by this sweep.
        let allocation = if state.stopped || !observed.running {
            Allocation {
                running: false,
                cpu_millis: 0,
                memory_mib: 0,
                disk_gib: observed.disk_gib,
            }
        } else {
            observed
        };
        let result = if !replacement_pending && (allocation.running || allocation.disk_gib > 0) {
            self.fund(&sandbox, &mut state, allocation, seconds(now))
                .await
        } else {
            Ok(())
        };
        if let Err(error) = result {
            self.failure(&sandbox, &mut state, &error, now).await?;
            return Err(error);
        }
        if !replacement_pending && sandbox.runtime.is_some() && !state.stopped {
            if let Some(receipt) = &state.current {
                if receipt.covered_until > seconds(now) {
                    let runtime = FundingRuntime::for_state(&state, &sandbox)?
                        .ok_or(FundingError::Conflict)?
                        .confirmed(receipt)?;
                    if let Err(error) = self.control.deadline(runtime).await {
                        self.failure(&sandbox, &mut state, &error, now).await?;
                        return Err(error);
                    }
                }
            }
        }
        state.retry_at = seconds(now).saturating_add(5);
        self.state.save(&mut state).await
    }
}

#[async_trait]
impl SandboxFundingAdapter for ProviderFundingAdapter {
    async fn ensure_coverage(
        &self,
        sandbox: &Sandbox,
        now: SystemTime,
    ) -> Result<Option<u64>, FundingError> {
        let started = std::time::Instant::now();
        let generation = generation(sandbox);
        let mut state = match self.state.load(&sandbox.id).await? {
            Some(state) => state,
            None => {
                let mut state = SandboxBillingState::new(sandbox, generation);
                self.state.save(&mut state).await?;
                state
            }
        };
        if state.owner_id != sandbox.owner.as_str() || state.sandbox_id != sandbox.id.as_str() {
            return Err(FundingError::Invalid("funding owner changed".into()));
        }
        if state.unbilled_generation == Some(generation) {
            return Ok(None);
        }
        if state.stopped && state.runtime_generation == generation {
            return Err(FundingError::Unfunded(
                "stopped compute requires a new runtime generation".into(),
            ));
        }
        if state.retry_at > seconds(now) && state.last_error.is_some() {
            return Err(FundingError::Retryable("funding retry is scheduled".into()));
        }
        if let Err(error) = self
            .fund(
                sandbox,
                &mut state,
                maximum_allocation(sandbox),
                seconds(now),
            )
            .await
        {
            self.failure(sandbox, &mut state, &error, now).await?;
            return Err(error);
        }
        if state.unbilled_generation == Some(generation) {
            return Ok(None);
        }
        let receipt = state
            .current
            .as_ref()
            .ok_or_else(|| FundingError::Invalid("funding receipt missing".into()))?;
        let observed_now = seconds(now).saturating_add(started.elapsed().as_secs());
        if state.coverage_start > observed_now
            || receipt.covered_until <= observed_now.saturating_add(2)
        {
            return Err(FundingError::Retryable(
                "confirmed coverage is not live".into(),
            ));
        }
        if sandbox.runtime.is_some() {
            let runtime = FundingRuntime::for_state(&state, sandbox)?
                .ok_or(FundingError::Conflict)?
                .confirmed(receipt)?;
            self.control.deadline(runtime).await?;
        }
        Ok(Some(receipt.covered_until))
    }

    async fn settle(&self, sandbox: &Sandbox, now: SystemTime) -> Result<(), FundingError> {
        if let Some(mut state) = self.state.load(&sandbox.id).await? {
            if !runtime_scope::completion_applies(&state, sandbox)? {
                return Ok(());
            }
            if state.unbilled_generation.is_some() {
                return Ok(());
            }
            state.stopped = true;
            state.retry_at = seconds(now);
            self.state.save(&mut state).await?;
        }
        Ok(())
    }

    async fn renew_due(&self, now: SystemTime) -> Result<usize, FundingError> {
        use futures_util::{stream, StreamExt};
        let states = self.state.due(seconds(now)).await?;
        let mut results = stream::iter(states.into_iter().map(|state| self.reconcile(state, now)))
            .buffer_unordered(16);
        let mut completed = 0;
        while let Some(result) = results.next().await {
            match result {
                Ok(()) => completed += 1,
                Err(error) => tracing::warn!(%error, "Sandbox funding reconciliation will retry"),
            }
        }
        Ok(completed)
    }
}

pub(crate) fn production(
    config: Option<SandboxBillingConfig>,
    store: Arc<MongoSandboxStore>,
    adapter: Arc<dyn layer3_adapter::SandboxAdapter>,
    deployment: String,
) -> Result<Arc<dyn SandboxFundingAdapter>, String> {
    let Some(config) = config else {
        return Ok(Arc::new(NoopFundingAdapter));
    };
    tracing::info!(producer_id = %config.producer_id, "NyxID Sandbox funding enabled");
    Ok(ProviderFundingAdapter::new(
        NyxIdFundingProvider::new(config)?,
        Arc::new(mongo::MongoFundingStateStore(store.clone())),
        Arc::new(mongo::RuntimeControl {
            store,
            adapter,
            deployment,
        }),
    ))
}

pub(crate) struct FundingRenewalWorker {
    adapter: Arc<dyn SandboxFundingAdapter>,
}
impl FundingRenewalWorker {
    pub(crate) fn new(adapter: Arc<dyn SandboxFundingAdapter>) -> Self {
        Self { adapter }
    }
    pub(crate) fn spawn(
        self,
        mut stop: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        tokio::spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(5));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                tokio::select! {
                    _ = stop.changed() => break,
                    _ = tick.tick() => {
                        tokio::select! {
                            _ = stop.changed() => break,
                            result = self.adapter.renew_due(SystemTime::now()) => {
                                if let Err(error) = result { tracing::warn!(%error, "Sandbox funding sweep failed"); }
                            }
                        }
                    }
                }
            }
        })
    }
}
