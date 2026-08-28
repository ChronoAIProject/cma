//! Closed operational signals for CMA Trigger.
//!
//! Alert identifiers are a contract with deployment-owned rules and runbooks.
//! Keeping them here makes a source predicate choose from a reviewed set rather
//! than inventing a string during an incident. Two predicates necessarily live
//! outside this process: a cross-surface sensitive-data detector and a joined
//! session-ledger/runtime continuity detector. Volume detection has a third,
//! explicit boundary: CMA emits safe closed increments and the deployment owns
//! their cross-replica time window. The source classification below makes those
//! boundaries executable instead of implying CMA can observe evidence it does
//! not possess.

use layer2_core::{ActivityAction, ActivityOutcome, ActivityRecord, ActivityService, DetailKey};

pub(crate) const OBSERVABILITY_SCHEMA_VERSION: u64 = 1;

/// The only low-cardinality labels admitted to scheduler metric extraction.
pub(crate) const METRIC_LABELS: [&str; 2] = ["lane", "outcome"];

/// The only label admitted to Trigger activity-counter metric extraction.
pub(crate) const ACTIVITY_COUNTER_LABELS: [&str; 1] = ["activity_counter"];

#[allow(dead_code)] // Deployment-owned variants are exercised by the catalog contract test.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum AlertDetectionSource {
    /// CMA has the complete predicate and emits the candidate itself.
    InProcess,
    /// CMA emits closed increments; a deployment rule owns aggregate windows.
    ActivityCounter,
    /// A deployment rule must join or inspect evidence CMA deliberately lacks.
    DeploymentDetector,
}

/// Successful control-plane requests that contribute to the volume alert.
///
/// These are deliberately request counters, not claims about unique Launches
/// or provider resources. Idempotent replays can increment them again. The
/// responder compares an alert window with durable Launch and PR records.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerActivityCounter {
    LaunchCreateSucceeded,
    PullRequestOpenSucceeded,
}

impl TriggerActivityCounter {
    #[cfg(test)]
    const ALL: [Self; 2] = [Self::LaunchCreateSucceeded, Self::PullRequestOpenSucceeded];

    pub(crate) const fn code(self) -> &'static str {
        match self {
            Self::LaunchCreateSucceeded => "launch_create_succeeded",
            Self::PullRequestOpenSucceeded => "pull_request_open_succeeded",
        }
    }
}

/// Classify one safe activity increment without inspecting identity, request
/// detail, repository data, or provider evidence.
fn trigger_activity_counter(record: &ActivityRecord) -> Option<TriggerActivityCounter> {
    if record.outcome != ActivityOutcome::Succeeded {
        return None;
    }
    match record.action {
        ActivityAction::TriggerLaunchCreate => Some(TriggerActivityCounter::LaunchCreateSucceeded),
        ActivityAction::TriggerLaunchPullRequestOpen => {
            Some(TriggerActivityCounter::PullRequestOpenSucceeded)
        }
        _ => None,
    }
}

#[allow(dead_code)] // External detector variants are not emitted by this process by design.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum TriggerAlert {
    SecurityLeak,
    SessionContinuity,
    DuplicateDispatch,
    UnauthorizedEffect,
    LaunchPullRequestVolume,
    AuditSink,
    CapabilityBound,
    StuckLaunch,
    SchedulerHealth,
    ProviderDegraded,
    CleanupDebt,
}

impl TriggerAlert {
    #[allow(dead_code)] // Read by the executable catalog contract test.
    pub(crate) const ALL: [Self; 11] = [
        Self::SecurityLeak,
        Self::SessionContinuity,
        Self::DuplicateDispatch,
        Self::UnauthorizedEffect,
        Self::LaunchPullRequestVolume,
        Self::AuditSink,
        Self::CapabilityBound,
        Self::StuckLaunch,
        Self::SchedulerHealth,
        Self::ProviderDegraded,
        Self::CleanupDebt,
    ];

    pub(crate) const fn id(self) -> &'static str {
        match self {
            Self::SecurityLeak => "TRIGGER-SECURITY-LEAK",
            Self::SessionContinuity => "TRIGGER-SESSION-CONTINUITY",
            Self::DuplicateDispatch => "TRIGGER-DUPLICATE-DISPATCH",
            Self::UnauthorizedEffect => "TRIGGER-UNAUTHORIZED-EFFECT",
            Self::LaunchPullRequestVolume => "TRIGGER-LAUNCH-PR-VOLUME",
            Self::AuditSink => "TRIGGER-AUDIT-SINK",
            Self::CapabilityBound => "TRIGGER-CAPABILITY-BOUND",
            Self::StuckLaunch => "TRIGGER-STUCK-LAUNCH",
            Self::SchedulerHealth => "TRIGGER-SCHEDULER-HEALTH",
            Self::ProviderDegraded => "TRIGGER-PROVIDER-DEGRADED",
            Self::CleanupDebt => "TRIGGER-CLEANUP-DEBT",
        }
    }

    pub(crate) const fn severity(self) -> &'static str {
        match self {
            Self::SecurityLeak
            | Self::SessionContinuity
            | Self::DuplicateDispatch
            | Self::UnauthorizedEffect => "SEV-0",
            Self::LaunchPullRequestVolume
            | Self::AuditSink
            | Self::CapabilityBound
            | Self::StuckLaunch
            | Self::SchedulerHealth => "SEV-1",
            Self::ProviderDegraded | Self::CleanupDebt => "SEV-2",
        }
    }

    #[allow(dead_code)] // Read by the executable catalog contract test.
    pub(crate) const fn detection_source(self) -> AlertDetectionSource {
        match self {
            Self::SecurityLeak | Self::SessionContinuity => {
                AlertDetectionSource::DeploymentDetector
            }
            Self::LaunchPullRequestVolume => AlertDetectionSource::ActivityCounter,
            Self::DuplicateDispatch
            | Self::UnauthorizedEffect
            | Self::AuditSink
            | Self::CapabilityBound
            | Self::StuckLaunch
            | Self::SchedulerHealth
            | Self::ProviderDegraded
            | Self::CleanupDebt => AlertDetectionSource::InProcess,
        }
    }
}

/// Persist one activity record, emit safe volume increments, and emit the
/// required Trigger sink-failure candidate without changing the user action.
pub(crate) async fn record_activity(activity: &ActivityService, record: ActivityRecord) {
    let activity_counter = trigger_activity_counter(&record);
    let is_trigger = record.action.is_trigger();
    let action = record.action;
    let actor = record
        .actor
        .as_ref()
        .map(|value| value.as_str())
        .unwrap_or("none")
        .to_owned();
    let target = record
        .target
        .as_ref()
        .map(|value| value.id())
        .unwrap_or("none")
        .to_owned();
    let correlation_id = record
        .detail
        .get(DetailKey::CorrelationId)
        .unwrap_or("none")
        .to_owned();
    let recorded = activity.record(record).await;
    if let Some(counter) = activity_counter {
        debug_assert_eq!(ACTIVITY_COUNTER_LABELS, ["activity_counter"]);
        tracing::info!(
            event_name = "cma_trigger_activity_counter",
            schema_version = OBSERVABILITY_SCHEMA_VERSION,
            activity_counter = counter.code(),
            increment = 1_u64,
            "Trigger activity counter increment observed"
        );
    }
    if !recorded && is_trigger {
        let alert = TriggerAlert::AuditSink;
        tracing::error!(
            event_name = "cma_trigger_alert_candidate",
            schema_version = OBSERVABILITY_SCHEMA_VERSION,
            alert_id = alert.id(),
            severity = alert.severity(),
            action = ?action,
            actor,
            target,
            correlation_id,
            "Trigger activity sink failure alert candidate observed"
        );
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn alert_catalog_and_detection_ownership_are_closed() {
        assert_eq!(
            TriggerAlert::ALL.map(TriggerAlert::id),
            [
                "TRIGGER-SECURITY-LEAK",
                "TRIGGER-SESSION-CONTINUITY",
                "TRIGGER-DUPLICATE-DISPATCH",
                "TRIGGER-UNAUTHORIZED-EFFECT",
                "TRIGGER-LAUNCH-PR-VOLUME",
                "TRIGGER-AUDIT-SINK",
                "TRIGGER-CAPABILITY-BOUND",
                "TRIGGER-STUCK-LAUNCH",
                "TRIGGER-SCHEDULER-HEALTH",
                "TRIGGER-PROVIDER-DEGRADED",
                "TRIGGER-CLEANUP-DEBT",
            ]
        );
        assert_eq!(
            TriggerAlert::ALL.map(TriggerAlert::detection_source),
            [
                AlertDetectionSource::DeploymentDetector,
                AlertDetectionSource::DeploymentDetector,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::ActivityCounter,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::InProcess,
                AlertDetectionSource::InProcess,
            ]
        );
        assert_eq!(TriggerAlert::LaunchPullRequestVolume.severity(), "SEV-1");
    }

    #[test]
    fn metric_labels_and_activity_counters_are_closed_and_low_cardinality() {
        assert_eq!(METRIC_LABELS, ["lane", "outcome"]);
        assert_eq!(ACTIVITY_COUNTER_LABELS, ["activity_counter"]);
        assert_eq!(
            TriggerActivityCounter::ALL.map(TriggerActivityCounter::code),
            ["launch_create_succeeded", "pull_request_open_succeeded"]
        );
    }

    #[test]
    fn activity_volume_counts_only_successful_launch_create_and_pr_open_requests() {
        for (action, expected) in [
            (
                ActivityAction::TriggerLaunchCreate,
                TriggerActivityCounter::LaunchCreateSucceeded,
            ),
            (
                ActivityAction::TriggerLaunchPullRequestOpen,
                TriggerActivityCounter::PullRequestOpenSucceeded,
            ),
        ] {
            let record = ActivityRecord::new(action, ActivityOutcome::Succeeded);
            assert_eq!(trigger_activity_counter(&record), Some(expected));

            let refused = ActivityRecord::new(action, ActivityOutcome::Rejected);
            assert_eq!(trigger_activity_counter(&refused), None);
        }

        for action in [
            ActivityAction::TriggerLaunchRead,
            ActivityAction::TriggerLaunchPullRequestReview,
            ActivityAction::TriggerLaunchPullRequestUpdate,
            ActivityAction::SessionCreate,
        ] {
            let record = ActivityRecord::new(action, ActivityOutcome::Succeeded);
            assert_eq!(trigger_activity_counter(&record), None, "{action:?}");
        }
    }
}
