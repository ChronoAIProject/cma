//! Bounded, database-led scheduling for Trigger orchestration and maintenance.
//!
//! The three lanes are deliberately independent. A slow GitHub write must not
//! delay capability revocation, and retention work must not widen either lane's
//! provider concurrency. Every candidate is still fenced by its Launch lease;
//! pages are only a bounded inventory and never an ownership claim.

use std::collections::BTreeSet;
use std::sync::Arc;
use std::time::{Duration, Instant, SystemTime};

use async_trait::async_trait;
use futures_util::{stream, StreamExt as _};
use layer2_core::{
    rfc3339_at, ActivityAction, ActivityOutcome, ActivityService, DetailKey, Launch, LaunchId,
    TriggerPage, TriggerPageRequest, TriggerService, TriggerServiceError,
};
use tokio::time::MissedTickBehavior;

use crate::config::{TriggerCleanupMode, TriggerRuntimeConfig};

use super::capability_worker::{CapabilityExecutor, CapabilityRunOutcome, CapabilityWorker};
use super::github_effects::GithubEffectExecutor;
use super::worker::{worker_record, EffectExecutor, EffectRunOutcome, EffectWorker};

const DEFAULT_WORKER_LEASE: Duration = Duration::from_secs(120);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum SchedulerLane {
    Effects,
    Capabilities,
    Cleanup,
}

impl SchedulerLane {
    fn code(self) -> &'static str {
        match self {
            Self::Effects => "effects",
            Self::Capabilities => "capabilities",
            Self::Cleanup => "cleanup",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CandidateOutcome {
    NotAcquired,
    Paused,
    NoWork,
    Completed,
    Deferred,
    Failed,
}

impl From<EffectRunOutcome> for CandidateOutcome {
    fn from(value: EffectRunOutcome) -> Self {
        match value {
            EffectRunOutcome::NotAcquired => Self::NotAcquired,
            EffectRunOutcome::Paused => Self::Paused,
            EffectRunOutcome::NoWork => Self::NoWork,
            EffectRunOutcome::Completed => Self::Completed,
            EffectRunOutcome::Deferred => Self::Deferred,
            EffectRunOutcome::Failed => Self::Failed,
        }
    }
}

impl From<CapabilityRunOutcome> for CandidateOutcome {
    fn from(value: CapabilityRunOutcome) -> Self {
        match value {
            CapabilityRunOutcome::NotAcquired => Self::NotAcquired,
            CapabilityRunOutcome::NoWork => Self::NoWork,
            CapabilityRunOutcome::Renewed | CapabilityRunOutcome::Revoked => Self::Completed,
            CapabilityRunOutcome::Deferred => Self::Deferred,
            CapabilityRunOutcome::Failed => Self::Failed,
        }
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
struct PassStats {
    pages: usize,
    candidates: usize,
    completed: usize,
    deferred: usize,
    paused: usize,
    failed: usize,
    not_acquired: usize,
    no_work: usize,
    errors: usize,
    page_budget_exhausted: bool,
    elapsed_milliseconds: u64,
}

impl PassStats {
    fn observe(&mut self, outcome: CandidateOutcome) {
        match outcome {
            CandidateOutcome::NotAcquired => self.not_acquired += 1,
            CandidateOutcome::Paused => self.paused += 1,
            CandidateOutcome::NoWork => self.no_work += 1,
            CandidateOutcome::Completed => self.completed += 1,
            CandidateOutcome::Deferred => self.deferred += 1,
            CandidateOutcome::Failed => self.failed += 1,
        }
    }

    fn finish(&mut self, started: Instant) {
        self.elapsed_milliseconds = started.elapsed().as_millis().min(u128::from(u64::MAX)) as u64;
    }
}

#[derive(Debug, Clone, Copy)]
struct SchedulerSettings {
    interval: Duration,
    page_size: u32,
    max_pages: usize,
    concurrency: usize,
}

impl From<&TriggerRuntimeConfig> for SchedulerSettings {
    fn from(config: &TriggerRuntimeConfig) -> Self {
        Self {
            interval: config.interval,
            page_size: config.page_size,
            max_pages: config.max_pages,
            concurrency: config.concurrency,
        }
    }
}

#[async_trait]
trait SchedulerBackend: Send + Sync {
    async fn candidates(
        &self,
        lane: SchedulerLane,
        pass_boundary: &str,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError>;

    async fn run_candidate(
        &self,
        lane: SchedulerLane,
        launch: Launch,
        correlation_id: String,
    ) -> Result<CandidateOutcome, TriggerServiceError>;
}

struct ProductionBackend {
    triggers: Arc<TriggerService>,
    effects: Arc<EffectWorker>,
    capabilities: Arc<CapabilityWorker>,
    cleanup: Arc<CleanupPlanner>,
    cleanup_mode: TriggerCleanupMode,
}

#[async_trait]
impl SchedulerBackend for ProductionBackend {
    async fn candidates(
        &self,
        lane: SchedulerLane,
        pass_boundary: &str,
        page: TriggerPageRequest,
    ) -> Result<TriggerPage<Launch>, TriggerServiceError> {
        match lane {
            SchedulerLane::Effects => {
                self.triggers
                    .worker_candidates(Some(pass_boundary.to_string()), page)
                    .await
            }
            SchedulerLane::Capabilities => self.triggers.capability_candidates(page).await,
            SchedulerLane::Cleanup => self.triggers.cleanup_candidates(pass_boundary, &page).await,
        }
    }

    async fn run_candidate(
        &self,
        lane: SchedulerLane,
        launch: Launch,
        correlation_id: String,
    ) -> Result<CandidateOutcome, TriggerServiceError> {
        match lane {
            SchedulerLane::Effects => Ok(self
                .effects
                .run_once(&launch.id, launch.revision_number, &correlation_id)
                .await?
                .into()),
            SchedulerLane::Capabilities => Ok(self
                .capabilities
                .run_once(&launch.id, launch.revision_number, &correlation_id)
                .await?
                .into()),
            SchedulerLane::Cleanup => match self.cleanup_mode {
                TriggerCleanupMode::DryRun if launch.cleanup.is_some() => {
                    Ok(CandidateOutcome::NoWork)
                }
                TriggerCleanupMode::DryRun => Ok(self
                    .cleanup
                    .run_once(&launch.id, launch.revision_number, &correlation_id)
                    .await?
                    .into()),
                TriggerCleanupMode::Execute => Ok(self
                    .effects
                    .run_once(&launch.id, launch.revision_number, &correlation_id)
                    .await?
                    .into()),
            },
        }
    }
}

struct TriggerScheduler {
    backend: Arc<dyn SchedulerBackend>,
    settings: SchedulerSettings,
}

impl TriggerScheduler {
    async fn run_pass(&self, lane: SchedulerLane) -> PassStats {
        let started = Instant::now();
        let pass_boundary = rfc3339_at(SystemTime::now());
        let mut stats = PassStats::default();
        let mut cursor = None;
        let mut seen_cursors = BTreeSet::new();

        for page_index in 0..self.settings.max_pages {
            let page = match self
                .backend
                .candidates(
                    lane,
                    &pass_boundary,
                    TriggerPageRequest::new(self.settings.page_size, cursor.clone()),
                )
                .await
            {
                Ok(page) => page,
                Err(error) => {
                    stats.errors += 1;
                    tracing::error!(
                        lane = lane.code(),
                        error = %error,
                        "Trigger scheduler could not read a candidate page"
                    );
                    break;
                }
            };
            stats.pages += 1;
            stats.candidates += page.items.len();

            let outcomes = stream::iter(page.items)
                .map(|launch| {
                    let backend = Arc::clone(&self.backend);
                    let correlation_id = scheduler_correlation(lane);
                    async move {
                        let launch_id = launch.id.clone();
                        let result = backend
                            .run_candidate(lane, launch, correlation_id.clone())
                            .await;
                        (launch_id, correlation_id, result)
                    }
                })
                .buffer_unordered(self.settings.concurrency)
                .collect::<Vec<_>>()
                .await;
            for (launch_id, correlation_id, result) in outcomes {
                match result {
                    Ok(outcome) => stats.observe(outcome),
                    Err(error) => {
                        stats.errors += 1;
                        tracing::error!(
                            lane = lane.code(),
                            launch_id = %launch_id.0,
                            correlation_id,
                            error = %error,
                            "Trigger scheduler candidate failed outside a settled worker outcome"
                        );
                    }
                }
            }

            let Some(next) = page.next else {
                break;
            };
            if !seen_cursors.insert(next.clone()) {
                stats.errors += 1;
                tracing::error!(
                    lane = lane.code(),
                    "Trigger scheduler stopped a repeated page cursor"
                );
                break;
            }
            if page_index + 1 == self.settings.max_pages {
                stats.page_budget_exhausted = true;
                break;
            }
            cursor = Some(next);
        }

        stats.finish(started);
        log_pass(lane, stats);
        stats
    }

    fn spawn_lane(self: &Arc<Self>, lane: SchedulerLane) -> tokio::task::JoinHandle<()> {
        let scheduler = Arc::clone(self);
        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(scheduler.settings.interval);
            ticker.set_missed_tick_behavior(MissedTickBehavior::Skip);
            loop {
                ticker.tick().await;
                scheduler.run_pass(lane).await;
            }
        })
    }
}

fn log_pass(lane: SchedulerLane, stats: PassStats) {
    let actionable = stats.completed > 0
        || stats.deferred > 0
        || stats.paused > 0
        || stats.failed > 0
        || stats.errors > 0
        || stats.page_budget_exhausted;
    // Every pass is an info-level heartbeat. This is intentionally structured
    // and bounded so an operator can distinguish an empty queue from a dead
    // scheduler without enabling content-bearing debug logs process-wide.
    tracing::info!(
        lane = lane.code(),
        pages = stats.pages,
        candidates = stats.candidates,
        completed = stats.completed,
        deferred = stats.deferred,
        paused = stats.paused,
        failed = stats.failed,
        not_acquired = stats.not_acquired,
        no_work = stats.no_work,
        errors = stats.errors,
        page_budget_exhausted = stats.page_budget_exhausted,
        actionable,
        elapsed_milliseconds = stats.elapsed_milliseconds,
        "Trigger scheduler pass settled"
    );
}

fn scheduler_correlation(lane: SchedulerLane) -> String {
    format!("trigger-{}-{}", lane.code(), uuid::Uuid::new_v4().simple())
}

trait CleanupClock: Send + Sync {
    fn now(&self) -> SystemTime;
}

struct SystemCleanupClock;

impl CleanupClock for SystemCleanupClock {
    fn now(&self) -> SystemTime {
        SystemTime::now()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum CleanupPlanOutcome {
    NotAcquired,
    NoWork,
    Planned,
}

impl From<CleanupPlanOutcome> for CandidateOutcome {
    fn from(value: CleanupPlanOutcome) -> Self {
        match value {
            CleanupPlanOutcome::NotAcquired => Self::NotAcquired,
            CleanupPlanOutcome::NoWork => Self::NoWork,
            CleanupPlanOutcome::Planned => Self::Completed,
        }
    }
}

/// Dry-run retention planner. It has no object-store or provider port, so its
/// type cannot delete a repository, sandbox, snapshot, or any other resource.
struct CleanupPlanner {
    triggers: Arc<TriggerService>,
    holder: String,
    clock: Arc<dyn CleanupClock>,
    lease_duration: Duration,
    activity: Arc<ActivityService>,
}

impl CleanupPlanner {
    fn new(
        triggers: Arc<TriggerService>,
        holder: impl Into<String>,
        activity: Arc<ActivityService>,
    ) -> Result<Self, TriggerServiceError> {
        let holder = holder.into();
        if holder.trim().is_empty() || holder.len() > 256 {
            return Err(layer2_core::TriggerValidationError::field(
                "cleanup_worker.holder",
                "is not usable",
            )
            .into());
        }
        Ok(Self {
            triggers,
            holder,
            clock: Arc::new(SystemCleanupClock),
            lease_duration: DEFAULT_WORKER_LEASE,
            activity,
        })
    }

    #[cfg(test)]
    fn with_clock(
        triggers: Arc<TriggerService>,
        holder: impl Into<String>,
        activity: Arc<ActivityService>,
        clock: Arc<dyn CleanupClock>,
    ) -> Result<Self, TriggerServiceError> {
        let mut planner = Self::new(triggers, holder, activity)?;
        planner.clock = clock;
        Ok(planner)
    }

    async fn run_once(
        &self,
        id: &LaunchId,
        expected_revision: u64,
        correlation_id: &str,
    ) -> Result<CleanupPlanOutcome, TriggerServiceError> {
        let acquired = self.clock.now();
        let now = rfc3339_at(acquired);
        let expires_at = rfc3339_at(acquired + self.lease_duration);
        let Some(mut launch) = self
            .triggers
            .acquire_worker(id, expected_revision, &self.holder, &now, &expires_at)
            .await?
        else {
            return Ok(CleanupPlanOutcome::NotAcquired);
        };
        let lease = launch
            .lease
            .clone()
            .expect("a cleanup planner owns a persisted Launch lease");

        if launch.cleanup.is_some() || !launch.cleanup_due(&now)? {
            self.triggers
                .release_worker(&launch.id, launch.revision_number, &lease)
                .await?;
            return Ok(CleanupPlanOutcome::NoWork);
        }

        let plan = match launch.plan_cleanup(&lease, &now, correlation_id) {
            Ok(plan) => plan,
            Err(error) => {
                let _ = self
                    .triggers
                    .release_worker(&launch.id, launch.revision_number, &lease)
                    .await;
                return Err(error.into());
            }
        };
        let committed = self
            .triggers
            .commit_worker(launch.revision_number, &lease, launch)
            .await?;
        let released = self
            .triggers
            .release_worker(&committed.id, committed.revision_number, &lease)
            .await?;
        let target_count = plan.targets.len();
        tracing::info!(
            launch_id = %released.id.0,
            correlation_id,
            target_count,
            fence = lease.fencing_token,
            "Trigger cleanup dry-run retained an exact CMA-owned object plan"
        );
        self.activity
            .record(
                worker_record(
                    &released,
                    ActivityAction::TriggerCleanupPlan,
                    ActivityOutcome::Succeeded,
                    correlation_id,
                )
                .with(DetailKey::CleanupTargetCount, target_count.to_string())
                .with(DetailKey::FencingToken, lease.fencing_token.to_string()),
            )
            .await;
        Ok(CleanupPlanOutcome::Planned)
    }
}

/// Start all Trigger lanes for the lifetime of the control-plane process.
///
/// This is called only after the composition root has proved every effect,
/// capability, workspace, signing, and session prerequisite exists. The loops
/// still run while rollout admission is disabled: orchestration then records a
/// durable pause, while the capability lane enforces the kill switch.
pub(crate) fn spawn(
    triggers: Arc<TriggerService>,
    executor: Arc<GithubEffectExecutor>,
    activity: Arc<ActivityService>,
    deployment_id: &str,
    config: &TriggerRuntimeConfig,
) -> Result<(), TriggerServiceError> {
    let effect_executor: Arc<dyn EffectExecutor> = executor.clone();
    let capability_executor: Arc<dyn CapabilityExecutor> = executor;
    let effects = Arc::new(
        EffectWorker::new(
            Arc::clone(&triggers),
            effect_executor,
            format!("{deployment_id}:trigger-effects"),
        )?
        .with_activity(Arc::clone(&activity)),
    );
    let capabilities = Arc::new(
        CapabilityWorker::new(
            Arc::clone(&triggers),
            capability_executor,
            format!("{deployment_id}:trigger-capabilities"),
        )?
        .with_activity(Arc::clone(&activity)),
    );
    let cleanup = Arc::new(CleanupPlanner::new(
        Arc::clone(&triggers),
        format!("{deployment_id}:trigger-cleanup"),
        activity,
    )?);
    let scheduler = Arc::new(TriggerScheduler {
        backend: Arc::new(ProductionBackend {
            triggers,
            effects,
            capabilities,
            cleanup,
            cleanup_mode: config.cleanup_mode,
        }),
        settings: SchedulerSettings::from(config),
    });

    for lane in [
        SchedulerLane::Effects,
        SchedulerLane::Capabilities,
        SchedulerLane::Cleanup,
    ] {
        scheduler.spawn_lane(lane);
    }
    tracing::info!(
        interval_seconds = config.interval.as_secs(),
        page_size = config.page_size,
        max_pages = config.max_pages,
        concurrency = config.concurrency,
        cleanup_mode = config.cleanup_mode.code(),
        environment_enabled = config.rollout.environment_enabled,
        orchestration_paused = config.rollout.orchestration_paused,
        capability_kill_switch = config.rollout.capability_kill_switch,
        "Trigger scheduler started with independent fenced lanes"
    );
    Ok(())
}

#[cfg(test)]
#[path = "scheduler_tests.rs"]
mod tests;
