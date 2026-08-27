use std::collections::{btree_map::Entry, BTreeMap, BTreeSet};

use serde::{Deserialize, Serialize};

use super::{
    Launch, LaunchBlockedReason, LaunchEvent, LaunchEventKind, LaunchFailure, LaunchId,
    LaunchState, LaunchStep, PromptDelivery,
};
use crate::trigger::ReachStatus;

/// Compact authoritative state captured with every progress event.
///
/// Step rows remain deltas after the creation event so the bounded event log
/// does not duplicate the full progress table on every transition.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchProgressState {
    pub state: LaunchState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<LaunchBlockedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<LaunchFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state: Option<LaunchState>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<ReachStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<PromptDelivery>,
}

impl LaunchProgressState {
    /// Capture the compact public progress state after one aggregate mutation.
    pub fn from_launch(launch: &Launch) -> Self {
        Self {
            state: launch.state,
            blocked_reason: launch.blocked_reason,
            failure: launch.failure.clone(),
            resume_state: launch.resume_state,
            reach: launch.reach.clone(),
            delivery: launch.delivery.clone(),
        }
    }
}

/// Progress state reconstructed only from the append-only Launch event log.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct LaunchProgressProjection {
    pub launch_id: LaunchId,
    pub state: LaunchState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub blocked_reason: Option<LaunchBlockedReason>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub failure: Option<LaunchFailure>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub resume_state: Option<LaunchState>,
    pub steps: Vec<LaunchStep>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reach: Option<ReachStatus>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub delivery: Option<PromptDelivery>,
    pub last_sequence: u64,
}

impl LaunchProgressProjection {
    pub fn from_launch(launch: &Launch) -> Self {
        Self {
            launch_id: launch.id.clone(),
            state: launch.state,
            blocked_reason: launch.blocked_reason,
            failure: launch.failure.clone(),
            resume_state: launch.resume_state,
            steps: launch.steps.clone(),
            reach: launch.reach.clone(),
            delivery: launch.delivery.clone(),
            last_sequence: launch.event_high_water(),
        }
    }

    fn apply_state(&mut self, state: &LaunchProgressState, sequence: u64) {
        self.state = state.state;
        self.blocked_reason = state.blocked_reason;
        self.failure = state.failure.clone();
        self.resume_state = state.resume_state;
        self.reach = state.reach.clone();
        self.delivery = state.delivery.clone();
        self.last_sequence = sequence;
    }

    /// Verify that this event-derived projection is the aggregate's current
    /// public progress state. Provider records and worker leases deliberately
    /// remain outside the progress stream.
    pub fn matches_launch(&self, launch: &Launch) -> bool {
        self == &Self::from_launch(launch)
    }
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum LaunchReplayError {
    #[error("Launch progress stream is empty")]
    Empty,
    #[error("Launch progress sequence must start at one, found {0}")]
    InvalidFirstSequence(u64),
    #[error("Launch progress has conflicting duplicate sequence {0}")]
    ConflictingDuplicate(u64),
    #[error("Launch progress has a sequence gap: expected {expected}, found {actual}")]
    SequenceGap { expected: u64, actual: u64 },
    #[error("Launch progress contains events for more than one Launch")]
    MixedLaunches,
    #[error("Launch progress sequence one is not a creation event")]
    MissingCreation,
    #[error("Launch progress contains more than one creation event")]
    DuplicateCreation,
    #[error("Launch progress creation event has an invalid step table")]
    InvalidInitialSteps,
    #[error("Launch progress changed an unknown or mismatched step")]
    InvalidStepChange,
    #[error("Launch progress event {0} contradicts its authoritative state")]
    ContradictoryEvent(u64),
    #[error("Launch progress replay does not match the retained Launch projection")]
    ProjectionMismatch,
}

impl Launch {
    /// Reconstruct this Launch's current progress solely from its event log.
    pub fn replay_progress(&self) -> Result<LaunchProgressProjection, LaunchReplayError> {
        replay_launch_progress(self.events.clone())
    }

    /// Detect a mutation retained without a corresponding replayable progress
    /// event. Persistence and conformance checks can use this as an invariant.
    pub fn validate_progress_replay(&self) -> Result<(), LaunchReplayError> {
        let projection = self.replay_progress()?;
        if projection.matches_launch(self) {
            Ok(())
        } else {
            Err(LaunchReplayError::ProjectionMismatch)
        }
    }
}

/// Reconstruct a Launch progress projection from a complete event stream.
///
/// Delivery order is irrelevant. Identical duplicate envelopes are ignored;
/// conflicting duplicates and missing sequences are rejected rather than
/// silently producing a state that never existed.
pub fn replay_launch_progress(
    events: impl IntoIterator<Item = LaunchEvent>,
) -> Result<LaunchProgressProjection, LaunchReplayError> {
    let mut ordered = BTreeMap::new();
    for event in events {
        match ordered.entry(event.sequence) {
            Entry::Vacant(entry) => {
                entry.insert(event);
            }
            Entry::Occupied(entry) if entry.get() == &event => {}
            Entry::Occupied(_) => {
                return Err(LaunchReplayError::ConflictingDuplicate(event.sequence));
            }
        }
    }

    let Some((&first_sequence, first)) = ordered.first_key_value() else {
        return Err(LaunchReplayError::Empty);
    };
    if first_sequence != 1 {
        return Err(LaunchReplayError::InvalidFirstSequence(first_sequence));
    }
    let launch_id = first.launch_id.clone();
    let mut expected = 1_u64;
    for (&sequence, event) in &ordered {
        if sequence != expected {
            return Err(LaunchReplayError::SequenceGap {
                expected,
                actual: sequence,
            });
        }
        if event.launch_id != launch_id {
            return Err(LaunchReplayError::MixedLaunches);
        }
        expected = expected
            .checked_add(1)
            .ok_or(LaunchReplayError::SequenceGap {
                expected: u64::MAX,
                actual: u64::MAX,
            })?;
    }

    let LaunchEventKind::Created { steps } = &first.kind else {
        return Err(LaunchReplayError::MissingCreation);
    };
    validate_initial_steps(steps)?;
    if first.progress
        != (LaunchProgressState {
            state: LaunchState::Draft,
            blocked_reason: None,
            failure: None,
            resume_state: None,
            reach: None,
            delivery: None,
        })
    {
        return Err(LaunchReplayError::ContradictoryEvent(first.sequence));
    }
    let mut projection = LaunchProgressProjection {
        launch_id,
        state: first.progress.state,
        blocked_reason: first.progress.blocked_reason,
        failure: first.progress.failure.clone(),
        resume_state: first.progress.resume_state,
        steps: steps.clone(),
        reach: first.progress.reach.clone(),
        delivery: first.progress.delivery.clone(),
        last_sequence: first.sequence,
    };

    for event in ordered.values().skip(1) {
        validate_event(&projection, event)?;
        if let LaunchEventKind::StepChanged { step } = &event.kind {
            let Some(current) = projection
                .steps
                .iter_mut()
                .find(|current| current.key == step.key)
            else {
                return Err(LaunchReplayError::InvalidStepChange);
            };
            if current.sequence != step.sequence {
                return Err(LaunchReplayError::InvalidStepChange);
            }
            *current = step.clone();
        }
        projection.apply_state(&event.progress, event.sequence);
    }
    projection.steps.sort_by_key(|step| step.sequence);
    Ok(projection)
}

fn validate_initial_steps(steps: &[LaunchStep]) -> Result<(), LaunchReplayError> {
    if steps.is_empty() {
        return Err(LaunchReplayError::InvalidInitialSteps);
    }
    let mut keys = BTreeSet::new();
    let mut sequences = BTreeSet::new();
    for step in steps {
        if step.sequence == 0
            || step.status != super::LaunchStepStatus::Pending
            || step.started_at.is_some()
            || step.completed_at.is_some()
            || step.attempt != 0
            || !keys.insert(step.key)
            || !sequences.insert(step.sequence)
        {
            return Err(LaunchReplayError::InvalidInitialSteps);
        }
    }
    if sequences
        .iter()
        .copied()
        .ne(1..=u32::try_from(steps.len()).map_err(|_| LaunchReplayError::InvalidInitialSteps)?)
    {
        return Err(LaunchReplayError::InvalidInitialSteps);
    }
    Ok(())
}

fn validate_event(
    projection: &LaunchProgressProjection,
    event: &LaunchEvent,
) -> Result<(), LaunchReplayError> {
    let agrees = match &event.kind {
        LaunchEventKind::Created { .. } => return Err(LaunchReplayError::DuplicateCreation),
        LaunchEventKind::StateChanged { from, to } => {
            *from == projection.state && *to == event.progress.state
        }
        LaunchEventKind::Blocked { reason } => event.progress.blocked_reason == Some(*reason),
        LaunchEventKind::Resumed { reason } => {
            projection.blocked_reason == Some(*reason) && event.progress.blocked_reason.is_none()
        }
        LaunchEventKind::Failure { failure } => event.progress.failure.as_ref() == Some(failure),
        LaunchEventKind::DeliveryChanged { state } => event
            .progress
            .delivery
            .as_ref()
            .is_some_and(|delivery| delivery.state == *state),
        LaunchEventKind::ReachChanged { reach } => event.progress.reach.as_ref() == Some(reach),
        LaunchEventKind::Cancelled => event.progress.state == LaunchState::Cancelled,
        LaunchEventKind::DraftUpdated
        | LaunchEventKind::ConnectionSelected
        | LaunchEventKind::Started
        | LaunchEventKind::StepChanged { .. }
        | LaunchEventKind::EffectReconciled { .. }
        | LaunchEventKind::RecoveryAuthorized { .. }
        | LaunchEventKind::ConfirmationRecorded { .. }
        | LaunchEventKind::CleanupPlanned { .. }
        | LaunchEventKind::CleanupCompleted { .. } => true,
    };
    if agrees {
        Ok(())
    } else {
        Err(LaunchReplayError::ContradictoryEvent(event.sequence))
    }
}

#[cfg(test)]
mod tests {
    use std::collections::BTreeMap;

    use super::*;
    use crate::trigger::{FailureClass, LaunchStepKey, LaunchStepStatus};
    use crate::SubjectId;

    fn state(state: LaunchState) -> LaunchProgressState {
        LaunchProgressState {
            state,
            blocked_reason: None,
            failure: None,
            resume_state: None,
            reach: None,
            delivery: None,
        }
    }

    fn step(status: LaunchStepStatus) -> LaunchStep {
        LaunchStep {
            key: LaunchStepKey::Review,
            sequence: 1,
            status,
            started_at: None,
            completed_at: (status == LaunchStepStatus::Completed).then(|| "t3".into()),
            attempt: 0,
            safe_detail: String::new(),
            remediation: String::new(),
            external_resource_ids: BTreeMap::new(),
            worker_fencing_token: 0,
            correlation_id: "correlation".into(),
        }
    }

    fn event(sequence: u64, kind: LaunchEventKind, progress: LaunchProgressState) -> LaunchEvent {
        LaunchEvent {
            sequence,
            launch_id: LaunchId("lnch_replay".into()),
            occurred_at: format!("t{sequence}"),
            correlation_id: "correlation".into(),
            progress,
            kind,
        }
    }

    fn stream() -> Vec<LaunchEvent> {
        let pending = step(LaunchStepStatus::Pending);
        let completed = step(LaunchStepStatus::Completed);
        vec![
            event(
                1,
                LaunchEventKind::Created {
                    steps: vec![pending],
                },
                state(LaunchState::Draft),
            ),
            event(
                2,
                LaunchEventKind::StateChanged {
                    from: LaunchState::Draft,
                    to: LaunchState::Preflight,
                },
                state(LaunchState::Preflight),
            ),
            event(
                3,
                LaunchEventKind::StepChanged {
                    step: completed.clone(),
                },
                state(LaunchState::Preflight),
            ),
        ]
    }

    fn real_launch() -> Launch {
        Launch::new(
            SubjectId("replay-owner".into()),
            crate::trigger::tests::revision_fixture(SubjectId("replay-publisher".into())),
            "2026-08-26T12:00:00Z",
            "replay-create",
        )
        .expect("valid Launch")
    }

    #[test]
    fn replay_sorts_delivery_and_ignores_identical_duplicates() {
        let events = stream();
        let projection = replay_launch_progress([
            events[2].clone(),
            events[0].clone(),
            events[2].clone(),
            events[1].clone(),
        ])
        .expect("complete replay");

        assert_eq!(projection.launch_id, LaunchId("lnch_replay".into()));
        assert_eq!(projection.state, LaunchState::Preflight);
        assert_eq!(projection.steps, vec![step(LaunchStepStatus::Completed)]);
        assert_eq!(projection.last_sequence, 3);
    }

    #[test]
    fn replay_rejects_conflicting_duplicates() {
        let mut events = stream();
        let mut conflict = events[1].clone();
        conflict.correlation_id = "different".into();
        events.push(conflict);

        assert_eq!(
            replay_launch_progress(events),
            Err(LaunchReplayError::ConflictingDuplicate(2))
        );
    }

    #[test]
    fn replay_rejects_sequence_gaps() {
        let mut events = stream();
        events.remove(1);

        assert_eq!(
            replay_launch_progress(events),
            Err(LaunchReplayError::SequenceGap {
                expected: 2,
                actual: 3,
            })
        );
    }

    #[test]
    fn replay_rejects_event_state_contradictions() {
        let mut events = stream();
        events[1].progress.state = LaunchState::ForkPending;

        assert_eq!(
            replay_launch_progress(events),
            Err(LaunchReplayError::ContradictoryEvent(2))
        );
    }

    #[test]
    fn aggregate_creation_and_failures_replay_to_the_exact_retained_projection() {
        let mut launch = real_launch();
        launch.validate_progress_replay().expect("creation replays");

        launch
            .fail(
                LaunchFailure {
                    class: FailureClass::FeaturePaused,
                    code: "trigger_feature_paused".into(),
                    detail: "New Launch work is paused.".into(),
                    remediation: "Wait for the rollout owner to resume the feature.".into(),
                    correlation_id: "pause".into(),
                    retry_at: None,
                    observed_at: "2026-08-26T12:01:00Z".into(),
                },
                "2026-08-26T12:01:00Z",
            )
            .expect("pause is durable");
        launch
            .validate_progress_replay()
            .expect("blocked failure replays");

        launch
            .resume_feature_pause("2026-08-26T12:02:00Z", "resume")
            .expect("feature resumes");
        launch
            .validate_progress_replay()
            .expect("cleared failure replays");

        launch
            .fail(
                LaunchFailure {
                    class: FailureClass::RetryableTransient,
                    code: "provider_unavailable".into(),
                    detail: "The provider is temporarily unavailable.".into(),
                    remediation: "CMA will retry this retained operation.".into(),
                    correlation_id: "retryable".into(),
                    retry_at: Some("2026-08-26T12:04:00Z".into()),
                    observed_at: "2026-08-26T12:03:00Z".into(),
                },
                "2026-08-26T12:03:00Z",
            )
            .expect("retryable failure is durable");
        launch
            .validate_progress_replay()
            .expect("retryable state replays");
    }

    #[test]
    fn replay_invariant_detects_unlogged_progress_mutation() {
        let mut launch = real_launch();
        launch.blocked_reason = Some(LaunchBlockedReason::RecoveryRequired);

        assert_eq!(
            launch.validate_progress_replay(),
            Err(LaunchReplayError::ProjectionMismatch)
        );
    }

    #[test]
    fn launch_event_sequence_continues_beyond_the_legacy_embedded_limit() {
        let mut launch = real_launch();
        for sequence in 0..2_100 {
            launch
                .update_overrides(
                    launch.overrides.clone(),
                    format!("2026-08-26T12:{:02}:00Z", sequence % 60),
                    format!("long-running-{sequence}"),
                )
                .expect("event allocation remains available");
        }

        assert_eq!(launch.event_high_water(), 2_101);
        assert_eq!(launch.events.len(), 2_101);
        launch
            .validate_progress_replay()
            .expect("the complete canonical stream still replays");
    }
}
