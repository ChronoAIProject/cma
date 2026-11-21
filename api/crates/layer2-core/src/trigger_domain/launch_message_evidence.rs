use crate::sandbox::{FirstMessageDelivery, FirstMessageRecord};
use crate::AgentId;

use super::launch_progress::progress_error;
use super::{Launch, LaunchEventKind, LaunchMessageEvidence, TriggerError};

#[derive(Clone, Copy)]
pub(super) enum EvidenceMode {
    Worker,
    Recovery,
}

pub(super) fn observe_first_message(
    launch: &mut Launch,
    agent_id: &AgentId,
    record: &FirstMessageRecord,
    mode: EvidenceMode,
) -> Result<Option<LaunchEventKind>, TriggerError> {
    validate_first_message_record(record)?;
    if launch.resolution.agent.id() != Some(agent_id) {
        return Err(progress_error());
    }
    let next = LaunchMessageEvidence::from_record(agent_id.clone(), record);
    validate_cursor_shape(&next, record.state, launch.input_protocol.is_some())?;
    if let Some(current) = &launch.first_message_evidence {
        validate_cursor_shape(
            current,
            launch.first_message,
            launch.input_protocol.is_some(),
        )?;
        validate_stable_identity(current, &next)?;
        validate_evidence_advance(current, &next, launch.first_message, record.state, mode)?;
    } else if launch.first_message != FirstMessageDelivery::Queued {
        return Err(progress_error());
    }
    if !delivery_can_advance(launch.first_message, record.state, mode) {
        return Err(progress_error());
    }
    let changed = launch.first_message != record.state;
    launch.first_message = record.state;
    launch.first_message_evidence = Some(next);
    Ok(changed.then_some(LaunchEventKind::FirstMessageChanged {
        state: record.state,
    }))
}

pub(super) fn validate_persisted_message_evidence(launch: &Launch) -> Result<(), TriggerError> {
    let Some(evidence) = &launch.first_message_evidence else {
        return matches!(
            launch.first_message,
            FirstMessageDelivery::Queued | FirstMessageDelivery::Completed
        )
        .then_some(())
        .ok_or_else(progress_error);
    };
    if launch.resolution.agent.id() != Some(&evidence.agent_id)
        || !valid_origin_key(&evidence.origin_key)
        || !valid_sha256(&evidence.payload_sha256)
    {
        return Err(progress_error());
    }
    validate_cursor_shape(
        evidence,
        launch.first_message,
        launch.input_protocol.is_some(),
    )
}

impl LaunchMessageEvidence {
    fn from_record(agent_id: AgentId, record: &FirstMessageRecord) -> Self {
        Self {
            agent_id,
            origin_key: record.origin_key.clone(),
            payload_sha256: record.payload_sha256.clone(),
            recovery_generation: record.recovery_generation,
            attempt: record.attempt,
            fence_sequence: record.fence_sequence,
            dispatch_generation: record.dispatch_generation,
            turn_sequence: record.turn_sequence,
        }
    }
}

fn validate_stable_identity(
    current: &LaunchMessageEvidence,
    next: &LaunchMessageEvidence,
) -> Result<(), TriggerError> {
    if current.agent_id != next.agent_id
        || current.origin_key != next.origin_key
        || current.payload_sha256 != next.payload_sha256
    {
        return Err(progress_error());
    }
    Ok(())
}

fn validate_evidence_advance(
    current: &LaunchMessageEvidence,
    next: &LaunchMessageEvidence,
    current_state: FirstMessageDelivery,
    next_state: FirstMessageDelivery,
    mode: EvidenceMode,
) -> Result<(), TriggerError> {
    if next.recovery_generation < current.recovery_generation
        || matches!(mode, EvidenceMode::Worker)
            && next.recovery_generation != current.recovery_generation
        || matches!(mode, EvidenceMode::Recovery)
            && next.recovery_generation == current.recovery_generation
            && !matches!(
                next_state,
                FirstMessageDelivery::ExecutionObserved
                    | FirstMessageDelivery::Completed
                    | FirstMessageDelivery::Failed
            )
        || next.attempt < current.attempt
    {
        return Err(progress_error());
    }

    if next.recovery_generation > current.recovery_generation {
        return validate_recovery_generation_advance(current, next, next_state);
    }
    if next.attempt > current.attempt {
        let current_can_retry = matches!(
            current_state,
            FirstMessageDelivery::Queued | FirstMessageDelivery::DispatchFenced
        );
        let turn_is_stable = current
            .turn_sequence
            .is_none_or(|turn| next.turn_sequence == Some(turn));
        return (current_can_retry
            && next.fence_sequence > current.fence_sequence
            && turn_is_stable)
            .then_some(())
            .ok_or_else(progress_error);
    }

    let proven_pre_write_retry = current_state == FirstMessageDelivery::DispatchFenced
        && next_state == FirstMessageDelivery::Queued
        && next.dispatch_generation == 0
        && next.fence_sequence == current.fence_sequence
        && next.turn_sequence == current.turn_sequence;
    if proven_pre_write_retry
        || next.fence_sequence == current.fence_sequence
            && next.dispatch_generation == current.dispatch_generation
            && next.turn_sequence == current.turn_sequence
    {
        Ok(())
    } else {
        Err(progress_error())
    }
}

fn validate_recovery_generation_advance(
    current: &LaunchMessageEvidence,
    next: &LaunchMessageEvidence,
    next_state: FirstMessageDelivery,
) -> Result<(), TriggerError> {
    if next.attempt == current.attempt {
        return (next_state == FirstMessageDelivery::Queued
            && next.fence_sequence == current.fence_sequence
            && next.dispatch_generation == 0
            && next.turn_sequence.is_none())
        .then_some(())
        .ok_or_else(progress_error);
    }
    let turn_advanced = match (current.turn_sequence, next.turn_sequence) {
        (Some(current), Some(next)) => next > current,
        (None, Some(next)) => next > 0,
        (_, None) => next_state == FirstMessageDelivery::Queued,
    };
    (next.fence_sequence > current.fence_sequence && turn_advanced)
        .then_some(())
        .ok_or_else(progress_error)
}

pub(super) fn validate_first_message_record(
    record: &FirstMessageRecord,
) -> Result<(), TriggerError> {
    record.validate().map_err(|_| progress_error())?;
    (valid_origin_key(&record.origin_key)
        && valid_sha256(&record.payload_sha256)
        && valid_cursor_values(
            record.recovery_generation,
            record.attempt,
            record.fence_sequence,
            record.dispatch_generation,
            record.turn_sequence,
            record.state,
            record.common_binding.is_some(),
        ))
    .then_some(())
    .ok_or_else(progress_error)
}

fn validate_cursor_shape(
    evidence: &LaunchMessageEvidence,
    state: FirstMessageDelivery,
    common: bool,
) -> Result<(), TriggerError> {
    valid_cursor_values(
        evidence.recovery_generation,
        evidence.attempt,
        evidence.fence_sequence,
        evidence.dispatch_generation,
        evidence.turn_sequence,
        state,
        common,
    )
    .then_some(())
    .ok_or_else(progress_error)
}

fn valid_cursor_values(
    recovery_generation: u32,
    attempt: u32,
    fence_sequence: u64,
    dispatch_generation: u64,
    turn_sequence: Option<u64>,
    state: FirstMessageDelivery,
    common: bool,
) -> bool {
    match state {
        FirstMessageDelivery::Failed if common && attempt == 0 => {
            recovery_generation == 0
                && fence_sequence == 0
                && dispatch_generation == 0
                && turn_sequence.is_none()
        }
        FirstMessageDelivery::Queued if attempt == 0 => {
            recovery_generation == 0
                && fence_sequence == 0
                && dispatch_generation == 0
                && turn_sequence.is_none()
        }
        FirstMessageDelivery::Queued => {
            fence_sequence > 0
                && dispatch_generation == 0
                && turn_sequence.is_none_or(|turn| turn > 0)
                && (recovery_generation > 0 || turn_sequence.is_some())
        }
        FirstMessageDelivery::DispatchFenced
        | FirstMessageDelivery::WriteConfirmed
        | FirstMessageDelivery::ExecutionObserved
        | FirstMessageDelivery::Completed
        | FirstMessageDelivery::Failed
        | FirstMessageDelivery::RecoveryRequired => {
            attempt > 0
                && fence_sequence > 0
                && dispatch_generation > 0
                && turn_sequence.is_some_and(|turn| turn > 0)
        }
    }
}

fn valid_origin_key(value: &str) -> bool {
    !value.trim().is_empty() && value.len() <= 256 && !value.chars().any(char::is_control)
}

fn valid_sha256(value: &str) -> bool {
    value.len() == 64
        && value
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}

fn delivery_can_advance(
    current: FirstMessageDelivery,
    next: FirstMessageDelivery,
    mode: EvidenceMode,
) -> bool {
    if current == next {
        return true;
    }
    if matches!(mode, EvidenceMode::Recovery) {
        return current == FirstMessageDelivery::RecoveryRequired
            && matches!(
                next,
                FirstMessageDelivery::Queued
                    | FirstMessageDelivery::DispatchFenced
                    | FirstMessageDelivery::WriteConfirmed
                    | FirstMessageDelivery::ExecutionObserved
                    | FirstMessageDelivery::Completed
                    | FirstMessageDelivery::Failed
            );
    }
    matches!(
        (current, next),
        (
            FirstMessageDelivery::Queued,
            FirstMessageDelivery::DispatchFenced
                | FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::Completed
                | FirstMessageDelivery::Failed
                | FirstMessageDelivery::RecoveryRequired
        ) | (
            FirstMessageDelivery::DispatchFenced,
            FirstMessageDelivery::Queued
                | FirstMessageDelivery::WriteConfirmed
                | FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::Completed
                | FirstMessageDelivery::Failed
                | FirstMessageDelivery::RecoveryRequired
        ) | (
            FirstMessageDelivery::WriteConfirmed,
            FirstMessageDelivery::ExecutionObserved
                | FirstMessageDelivery::Completed
                | FirstMessageDelivery::Failed
                | FirstMessageDelivery::RecoveryRequired
        ) | (
            FirstMessageDelivery::ExecutionObserved,
            FirstMessageDelivery::Completed | FirstMessageDelivery::Failed
        )
    )
}
