//! Bounded, independent recovery lanes for durable sponsorship work.

use std::collections::BTreeSet;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use futures_util::{stream, StreamExt as _};
use layer2_core::{
    Authorization, Caller, Capability, LaunchId, SponsoredCredentialId, SponsoredGrantId,
    SponsoredModelDataPlaneError, SponsoredModelDataPlaneService, SponsoredUsageReservationId,
    SponsoredUsageState, SponsorshipCiphertextRecordKind, SponsorshipCiphertextRewrapCandidate,
    SponsorshipError, SponsorshipRecoveryPage, SponsorshipRecoveryPageRequest, SponsorshipService,
    SponsorshipStore, SponsorshipStoreError, SubjectId, MAX_SPONSORED_AUDIT_PAGE,
};
use tokio::time::MissedTickBehavior;

const MAX_INTERVAL: Duration = Duration::from_secs(300);
const MAX_PAGES: usize = 100;
const MAX_CONCURRENCY: usize = 32;
const MAX_RETIRING_ENCRYPTION_KEYS: usize = 4;
// The deployment grants the whole process 20 seconds. Worker cancellation must
// complete inside the process-wide 15-second drain and leave shutdown headroom.
const WORKER_DRAIN_TIMEOUT: Duration = Duration::from_secs(10);

#[derive(Debug, thiserror::Error)]
pub(crate) enum SponsorshipWorkerError {
    #[error("invalid sponsorship worker configuration: {0}")]
    InvalidConfiguration(String),
    #[error("sponsorship recovery failed: {0}")]
    Sponsorship(#[from] SponsorshipError),
    #[error("sponsorship recovery persistence failed: {0}")]
    Store(#[from] SponsorshipStoreError),
    #[error("sponsored model recovery failed: {0}")]
    Model(#[from] SponsoredModelDataPlaneError),
}

/// Runtime bounds repeated at the worker boundary so direct construction
/// cannot bypass deployment configuration validation.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) struct SponsorshipWorkerSettings {
    interval: Duration,
    page_size: u32,
    max_pages: usize,
    concurrency: usize,
}

impl SponsorshipWorkerSettings {
    pub(crate) fn new(
        interval: Duration,
        page_size: u32,
        max_pages: usize,
        concurrency: usize,
    ) -> Result<Self, SponsorshipWorkerError> {
        if interval < Duration::from_secs(1) || interval > MAX_INTERVAL {
            return Err(SponsorshipWorkerError::InvalidConfiguration(
                "interval must be between 1 and 300 seconds".into(),
            ));
        }
        if !(1..=MAX_SPONSORED_AUDIT_PAGE as u32).contains(&page_size) {
            return Err(SponsorshipWorkerError::InvalidConfiguration(format!(
                "page size must be between 1 and {MAX_SPONSORED_AUDIT_PAGE}"
            )));
        }
        if !(1..=MAX_PAGES).contains(&max_pages) {
            return Err(SponsorshipWorkerError::InvalidConfiguration(
                "maximum pages must be between 1 and 100".into(),
            ));
        }
        if !(1..=MAX_CONCURRENCY).contains(&concurrency) {
            return Err(SponsorshipWorkerError::InvalidConfiguration(
                "concurrency must be between 1 and 32".into(),
            ));
        }
        Ok(Self {
            interval,
            page_size,
            max_pages,
            concurrency,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecoveryLane {
    Credentials,
    Activations,
    Grants,
    Usage,
    Rewrap {
        kind: SponsorshipCiphertextRecordKind,
        retiring_key_id: String,
    },
}

impl RecoveryLane {
    fn code(&self) -> &'static str {
        match self {
            Self::Credentials => "credentials",
            Self::Activations => "activations",
            Self::Grants => "grants",
            Self::Usage => "usage",
            Self::Rewrap { .. } => "ciphertext_rewrap",
        }
    }

    fn needs_time_fence(&self) -> bool {
        matches!(self, Self::Grants)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum RecoveryCandidate {
    Credential(SponsoredCredentialId),
    Activation(LaunchId),
    Grant(SponsoredGrantId),
    Usage(SponsoredUsageReservationId),
    Rewrap(SponsorshipCiphertextRewrapCandidate),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateOutcome {
    Completed,
    NoWork,
    Deferred,
    Unresolved,
}

impl CandidateOutcome {
    fn code(self) -> &'static str {
        match self {
            Self::Completed => "completed",
            Self::NoWork => "no_work",
            Self::Deferred => "deferred",
            Self::Unresolved => "unresolved",
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PassStats {
    pages: usize,
    candidates: usize,
    completed: usize,
    no_work: usize,
    deferred: usize,
    unresolved: usize,
    errors: usize,
    page_budget_exhausted: bool,
    elapsed_milliseconds: u64,
}

impl PassStats {
    fn observe(&mut self, outcome: CandidateOutcome) {
        match outcome {
            CandidateOutcome::Completed => self.completed += 1,
            CandidateOutcome::NoWork => self.no_work += 1,
            CandidateOutcome::Deferred => self.deferred += 1,
            CandidateOutcome::Unresolved => self.unresolved += 1,
        }
    }

    fn finish(&mut self, started: Instant) {
        self.elapsed_milliseconds = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    }
}

#[async_trait]
trait RecoveryBackend: Send + Sync {
    fn time_fence(&self) -> Result<String, SponsorshipWorkerError>;

    async fn candidates(
        &self,
        lane: &RecoveryLane,
        as_of: Option<&str>,
        page: SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<RecoveryCandidate>, SponsorshipWorkerError>;

    async fn run_candidate(
        &self,
        candidate: RecoveryCandidate,
        as_of: Option<&str>,
    ) -> Result<CandidateOutcome, SponsorshipWorkerError>;
}

struct ProductionBackend {
    service: Arc<SponsorshipService>,
    store: Arc<dyn SponsorshipStore>,
    data_plane: Arc<SponsoredModelDataPlaneService>,
    caller: Caller,
}

impl ProductionBackend {
    fn new(
        service: Arc<SponsorshipService>,
        store: Arc<dyn SponsorshipStore>,
        data_plane: Arc<SponsoredModelDataPlaneService>,
    ) -> Self {
        Self {
            service,
            store,
            data_plane,
            caller: recovery_caller(),
        }
    }
}

#[async_trait]
impl RecoveryBackend for ProductionBackend {
    fn time_fence(&self) -> Result<String, SponsorshipWorkerError> {
        self.service
            .sponsorship_recovery_time_fence(&self.caller)
            .map_err(Into::into)
    }

    async fn candidates(
        &self,
        lane: &RecoveryLane,
        as_of: Option<&str>,
        page: SponsorshipRecoveryPageRequest,
    ) -> Result<SponsorshipRecoveryPage<RecoveryCandidate>, SponsorshipWorkerError> {
        let (items, next) = match lane {
            RecoveryLane::Credentials => {
                let page = self.store.credential_recovery_candidates(&page).await?;
                (
                    page.items
                        .into_iter()
                        .map(RecoveryCandidate::Credential)
                        .collect(),
                    page.next,
                )
            }
            RecoveryLane::Activations => {
                let page = self.store.activation_recovery_candidates(&page).await?;
                (
                    page.items
                        .into_iter()
                        .map(RecoveryCandidate::Activation)
                        .collect(),
                    page.next,
                )
            }
            RecoveryLane::Grants => {
                let as_of = as_of.ok_or_else(|| {
                    SponsorshipWorkerError::InvalidConfiguration(
                        "grant recovery traversal has no time fence".into(),
                    )
                })?;
                let page = self.store.grant_recovery_candidates(as_of, &page).await?;
                (
                    page.items
                        .into_iter()
                        .map(RecoveryCandidate::Grant)
                        .collect(),
                    page.next,
                )
            }
            RecoveryLane::Usage => {
                let page = self.store.usage_recovery_candidates(&page).await?;
                (
                    page.items
                        .into_iter()
                        .map(RecoveryCandidate::Usage)
                        .collect(),
                    page.next,
                )
            }
            RecoveryLane::Rewrap {
                kind,
                retiring_key_id,
            } => {
                let page = self
                    .store
                    .ciphertext_rewrap_candidates(*kind, retiring_key_id, &page)
                    .await?;
                (
                    page.items
                        .into_iter()
                        .map(RecoveryCandidate::Rewrap)
                        .collect(),
                    page.next,
                )
            }
        };
        Ok(SponsorshipRecoveryPage { items, next })
    }

    async fn run_candidate(
        &self,
        candidate: RecoveryCandidate,
        as_of: Option<&str>,
    ) -> Result<CandidateOutcome, SponsorshipWorkerError> {
        let result = match candidate {
            RecoveryCandidate::Credential(id) => self
                .service
                .recover_credential_operation(&self.caller, &id)
                .await
                .map(|_| CandidateOutcome::Completed),
            RecoveryCandidate::Activation(id) => self
                .service
                .reconcile_activation_cleanup(&self.caller, &id)
                .await
                .map(|_| CandidateOutcome::Completed),
            RecoveryCandidate::Grant(id) => {
                let as_of = as_of.ok_or_else(|| {
                    SponsorshipWorkerError::InvalidConfiguration(
                        "grant recovery candidate has no time fence".into(),
                    )
                })?;
                self.service
                    .recover_grant(&self.caller, &id, as_of)
                    .await
                    .map(|_| CandidateOutcome::Completed)
            }
            RecoveryCandidate::Usage(id) => {
                return classify_usage_recovery(self.data_plane.recover_usage(&id).await);
            }
            RecoveryCandidate::Rewrap(candidate) => {
                return self
                    .store
                    .rewrap_ciphertext(&candidate)
                    .await
                    .map(|rewrapped| {
                        if rewrapped {
                            CandidateOutcome::Completed
                        } else {
                            CandidateOutcome::NoWork
                        }
                    })
                    .map_err(Into::into);
            }
        };
        match result {
            Ok(outcome) => Ok(outcome),
            Err(error) => classify_recovery_error(error),
        }
    }
}

fn classify_usage_recovery(
    result: Result<Option<SponsoredUsageState>, SponsoredModelDataPlaneError>,
) -> Result<CandidateOutcome, SponsorshipWorkerError> {
    match result {
        Ok(None) => Ok(CandidateOutcome::NoWork),
        Ok(Some(SponsoredUsageState::Settled | SponsoredUsageState::Released)) => {
            Ok(CandidateOutcome::Completed)
        }
        Ok(Some(
            SponsoredUsageState::Reserved
            | SponsoredUsageState::Attempting
            | SponsoredUsageState::Ambiguous,
        )) => Ok(CandidateOutcome::Deferred),
        Err(SponsoredModelDataPlaneError::Sponsorship(error))
        | Err(SponsoredModelDataPlaneError::Accounting(error)) => classify_recovery_error(error),
        Err(
            SponsoredModelDataPlaneError::ProviderUnavailable
            | SponsoredModelDataPlaneError::ProviderAmbiguous,
        ) => Ok(CandidateOutcome::Deferred),
        Err(SponsoredModelDataPlaneError::ProviderRejected) => Ok(CandidateOutcome::Unresolved),
        Err(SponsoredModelDataPlaneError::ReservationReleased) => Ok(CandidateOutcome::NoWork),
        Err(error @ SponsoredModelDataPlaneError::InvalidRequest(_)) => Err(error.into()),
    }
}

fn classify_recovery_error(
    error: SponsorshipError,
) -> Result<CandidateOutcome, SponsorshipWorkerError> {
    match error {
        SponsorshipError::NotFound | SponsorshipError::GrantUnavailable => {
            Ok(CandidateOutcome::NoWork)
        }
        SponsorshipError::ProviderAmbiguous
        | SponsorshipError::ProviderUnavailable
        | SponsorshipError::BillingAmbiguous
        | SponsorshipError::BillingUnavailable
        | SponsorshipError::ProviderOperationInProgress
        | SponsorshipError::ActivationInProgress
        | SponsorshipError::Store(SponsorshipStoreError::Conflict)
        | SponsorshipError::Store(SponsorshipStoreError::StaleGeneration) => {
            Ok(CandidateOutcome::Deferred)
        }
        SponsorshipError::ProviderRejected => Ok(CandidateOutcome::Unresolved),
        error => Err(error.into()),
    }
}

fn recovery_caller() -> Caller {
    Caller::new(
        SubjectId("cma-sponsorship-recovery".into()),
        Authorization::from_capabilities([Capability::ReconcileSponsoredUsage]),
    )
}

#[derive(Debug, Clone, Default)]
struct TraversalState {
    cursor: Option<String>,
    as_of: Option<String>,
}

struct RecoveryLaneScheduler {
    backend: Arc<dyn RecoveryBackend>,
    lane: RecoveryLane,
    settings: SponsorshipWorkerSettings,
    traversal: Mutex<TraversalState>,
}

impl RecoveryLaneScheduler {
    fn new(
        backend: Arc<dyn RecoveryBackend>,
        lane: RecoveryLane,
        settings: SponsorshipWorkerSettings,
    ) -> Self {
        Self {
            backend,
            lane,
            settings,
            traversal: Mutex::new(TraversalState::default()),
        }
    }

    async fn run_pass(&self) -> PassStats {
        let started = Instant::now();
        let mut stats = PassStats::default();
        let saved = self.traversal.lock().unwrap().clone();
        let mut cursor = saved.cursor;
        let as_of = if self.lane.needs_time_fence() {
            match saved.as_of {
                Some(as_of) if cursor.is_some() => Some(as_of),
                _ => match self.backend.time_fence() {
                    Ok(as_of) => Some(as_of),
                    Err(error) => {
                        stats.errors += 1;
                        tracing::error!(
                            lane = self.lane.code(),
                            error = %error,
                            "sponsorship recovery could not establish a traversal fence"
                        );
                        stats.finish(started);
                        log_pass(&self.lane, stats);
                        return stats;
                    }
                },
            }
        } else {
            None
        };
        let mut next_state = TraversalState {
            cursor: cursor.clone(),
            as_of: as_of.clone(),
        };
        let mut seen_cursors = cursor.iter().cloned().collect::<BTreeSet<_>>();

        for page_index in 0..self.settings.max_pages {
            let page = match self
                .backend
                .candidates(
                    &self.lane,
                    as_of.as_deref(),
                    SponsorshipRecoveryPageRequest::new(self.settings.page_size, cursor.clone()),
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    stats.errors += 1;
                    tracing::error!(
                        lane = self.lane.code(),
                        error = %error,
                        "sponsorship recovery could not read a candidate page"
                    );
                    break;
                }
            };
            stats.pages += 1;
            stats.candidates += page.items.len();

            let outcomes = stream::iter(page.items)
                .map(|candidate| {
                    let backend = Arc::clone(&self.backend);
                    let as_of = as_of.clone();
                    async move { backend.run_candidate(candidate, as_of.as_deref()).await }
                })
                .buffer_unordered(self.settings.concurrency)
                .collect::<Vec<_>>()
                .await;
            for outcome in outcomes {
                match outcome {
                    Ok(outcome) => {
                        stats.observe(outcome);
                        tracing::info!(
                            lane = self.lane.code(),
                            outcome = outcome.code(),
                            "sponsorship recovery candidate settled"
                        );
                    }
                    Err(error) => {
                        stats.errors += 1;
                        tracing::error!(
                            lane = self.lane.code(),
                            error = %error,
                            "sponsorship recovery candidate failed"
                        );
                    }
                }
            }

            let Some(next) = page.next else {
                next_state = TraversalState::default();
                break;
            };
            if !seen_cursors.insert(next.clone()) {
                stats.errors += 1;
                next_state = TraversalState::default();
                tracing::error!(
                    lane = self.lane.code(),
                    "sponsorship recovery stopped a repeated page cursor"
                );
                break;
            }
            next_state.cursor = Some(next.clone());
            if page_index + 1 == self.settings.max_pages {
                stats.page_budget_exhausted = true;
                break;
            }
            cursor = Some(next);
        }

        *self.traversal.lock().unwrap() = next_state;
        stats.finish(started);
        log_pass(&self.lane, stats);
        stats
    }

    fn spawn(
        self: &Arc<Self>,
        mut shutdown: tokio::sync::watch::Receiver<bool>,
    ) -> tokio::task::JoinHandle<()> {
        let scheduler = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(scheduler.settings.interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                if *shutdown.borrow() {
                    break;
                }
                tokio::select! {
                    biased;
                    changed = shutdown.changed() => {
                        if changed.is_err() || *shutdown.borrow() {
                            break;
                        }
                    }
                    _ = ticker.tick() => {
                        scheduler.run_pass().await;
                    }
                }
            }
        })
    }
}

fn log_pass(lane: &RecoveryLane, stats: PassStats) {
    tracing::info!(
        lane = lane.code(),
        pages = stats.pages,
        candidates = stats.candidates,
        completed = stats.completed,
        no_work = stats.no_work,
        deferred = stats.deferred,
        unresolved = stats.unresolved,
        errors = stats.errors,
        page_budget_exhausted = stats.page_budget_exhausted,
        elapsed_milliseconds = stats.elapsed_milliseconds,
        "sponsorship recovery pass settled"
    );
}

fn validate_retiring_key_ids(
    retiring_key_ids: Vec<String>,
) -> Result<Vec<String>, SponsorshipWorkerError> {
    if retiring_key_ids.len() > MAX_RETIRING_ENCRYPTION_KEYS {
        return Err(SponsorshipWorkerError::InvalidConfiguration(format!(
            "at most {MAX_RETIRING_ENCRYPTION_KEYS} retiring encryption keys are supported"
        )));
    }
    let mut unique = BTreeSet::new();
    for key_id in retiring_key_ids {
        if key_id.is_empty()
            || key_id.len() > 64
            || !key_id.bytes().all(|byte| {
                byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_' | b'.' | b':')
            })
        {
            return Err(SponsorshipWorkerError::InvalidConfiguration(
                "a retiring encryption key id must be 1 to 64 safe ASCII bytes".into(),
            ));
        }
        if !unique.insert(key_id.clone()) {
            return Err(SponsorshipWorkerError::InvalidConfiguration(
                "retiring encryption key ids must be unique".into(),
            ));
        }
    }
    Ok(unique.into_iter().collect())
}

/// Fully validated recovery workers that have not started provider or store work.
pub(crate) struct SponsorshipWorkerPlan {
    schedulers: Vec<Arc<RecoveryLaneScheduler>>,
    settings: SponsorshipWorkerSettings,
}

/// Owned recovery tasks for one running CMA process.
pub(crate) struct SponsorshipWorkers {
    shutdown: tokio::sync::watch::Sender<bool>,
    handles: Vec<tokio::task::JoinHandle<()>>,
}

impl SponsorshipWorkerPlan {
    /// Start every prepared lane after the process has committed to serving.
    pub(crate) fn start(self, shutdown: tokio::sync::watch::Sender<bool>) -> SponsorshipWorkers {
        let lane_count = self.schedulers.len();
        let handles = self
            .schedulers
            .into_iter()
            .map(|scheduler| scheduler.spawn(shutdown.subscribe()))
            .collect();
        tracing::info!(
            interval_seconds = self.settings.interval.as_secs(),
            page_size = self.settings.page_size,
            max_pages = self.settings.max_pages,
            concurrency = self.settings.concurrency,
            lane_count,
            "sponsorship recovery workers started"
        );
        SponsorshipWorkers { shutdown, handles }
    }
}

impl SponsorshipWorkers {
    /// Stop scheduling and give the active pass one bounded drain interval.
    pub(crate) async fn shutdown(mut self) {
        let _ = self.shutdown.send(true);
        let mut handles = std::mem::take(&mut self.handles);
        let drained = tokio::time::timeout(WORKER_DRAIN_TIMEOUT, async {
            for handle in &mut handles {
                if let Err(error) = handle.await {
                    tracing::error!(error = %error, "sponsorship recovery worker stopped unexpectedly");
                }
            }
        })
        .await
        .is_ok();
        if !drained {
            tracing::warn!(
                timeout_seconds = WORKER_DRAIN_TIMEOUT.as_secs(),
                "sponsorship recovery drain timed out; cancelling unfinished passes"
            );
            for handle in &handles {
                handle.abort();
            }
            for handle in handles {
                let _ = handle.await;
            }
        }
    }
}

impl Drop for SponsorshipWorkers {
    fn drop(&mut self) {
        let _ = self.shutdown.send(true);
        for handle in &self.handles {
            handle.abort();
        }
    }
}

/// Validate and prepare independent credential, activation, grant, usage, and
/// per-key ciphertext-rewrap lanes without starting any durable work.
pub(crate) fn prepare(
    service: Arc<SponsorshipService>,
    store: Arc<dyn SponsorshipStore>,
    data_plane: Arc<SponsoredModelDataPlaneService>,
    settings: SponsorshipWorkerSettings,
    retiring_encryption_key_ids: Vec<String>,
) -> Result<SponsorshipWorkerPlan, SponsorshipWorkerError> {
    let retiring_encryption_key_ids = validate_retiring_key_ids(retiring_encryption_key_ids)?;
    let backend: Arc<dyn RecoveryBackend> = Arc::new(ProductionBackend::new(
        service,
        Arc::clone(&store),
        data_plane,
    ));
    let mut lanes = vec![
        RecoveryLane::Credentials,
        RecoveryLane::Activations,
        RecoveryLane::Grants,
        RecoveryLane::Usage,
    ];
    for retiring_key_id in retiring_encryption_key_ids {
        for kind in [
            SponsorshipCiphertextRecordKind::Credential,
            SponsorshipCiphertextRecordKind::GrantActivation,
            SponsorshipCiphertextRecordKind::Grant,
            SponsorshipCiphertextRecordKind::UsageReservation,
        ] {
            lanes.push(RecoveryLane::Rewrap {
                kind,
                retiring_key_id: retiring_key_id.clone(),
            });
        }
    }
    let schedulers = lanes
        .into_iter()
        .map(|lane| {
            Arc::new(RecoveryLaneScheduler::new(
                Arc::clone(&backend),
                lane,
                settings,
            ))
        })
        .collect();
    Ok(SponsorshipWorkerPlan {
        schedulers,
        settings,
    })
}

#[cfg(test)]
#[path = "worker_tests.rs"]
mod tests;
