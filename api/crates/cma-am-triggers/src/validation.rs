use layer2_core::trigger_domain::{
    Launch, LaunchAdmission, LaunchEvent, LaunchEventKind, LaunchStoreError,
    ServiceConnectionHandoff, TriggerAvailability,
};

use super::corrupt;

pub fn is_ready_observation(
    current: &Launch,
    commit: &layer2_core::trigger_domain::LaunchCommit,
) -> bool {
    use layer2_core::trigger_domain::{LaunchState, LaunchWorkStep};
    if current.state != LaunchState::Ready
        || current.work.step != LaunchWorkStep::Finish
        || commit.expected_revision != current.revision
        || !commit.events.is_empty()
    {
        return false;
    }
    let Some(lease) = &current.work.lease else {
        return false;
    };
    let mut expected = current.clone();
    expected
        .defer_worker_launch(
            &lease.holder,
            lease.fence,
            commit.launch.work.ready_at.clone(),
            commit.event_at.clone(),
        )
        .is_ok_and(|events| events.is_empty())
        && expected == commit.launch
}

pub fn admission(admission: &LaunchAdmission) -> Result<(), LaunchStoreError> {
    admission.launch.validate()?;
    if admission.trigger.availability != TriggerAvailability::Published {
        return Err(LaunchStoreError::TriggerRetired);
    }
    if admission.launch.trigger_id != admission.trigger.trigger_id
        || admission.launch.public_id != admission.trigger.public_id
        || admission.launch.revision_digest != admission.trigger.revision.digest
    {
        return Err(LaunchStoreError::TriggerUnavailable);
    }
    if admission.trigger.revision.content.consent.always_review
        && admission.launch.started_at.is_none()
        && !admission.launch.confirmations.is_empty()
    {
        return Err(corrupt("publisher-required review cannot be preconfirmed"));
    }
    if admission.launch.event_head != 0 {
        return Err(corrupt("new Launch has a nonzero event head"));
    }
    if admission.events.is_empty()
        || !matches!(admission.events.first(), Some(LaunchEventKind::Created))
    {
        return Err(corrupt("Launch admission omitted its Created event"));
    }
    Ok(())
}

pub fn immutable_launch(current: &Launch, changed: &Launch) -> Result<(), LaunchStoreError> {
    if current.id != changed.id
        || current.trigger_id != changed.trigger_id
        || current.revision_digest != changed.revision_digest
        || current.public_id != changed.public_id
        || current.initiation != changed.initiation
        || current.input_protocol != changed.input_protocol
        || current.input_binding_sha256 != changed.input_binding_sha256
        || current.owner != changed.owner
        || current.authority != changed.authority
        || current.placement_id != changed.placement_id
        || current.referrer_origin != changed.referrer_origin
        || current.trust != changed.trust
        || !changed.confirmations.starts_with(&current.confirmations)
        || current.command_key_hash != changed.command_key_hash
        || current.command_fingerprint != changed.command_fingerprint
        || current.created_at != changed.created_at
        || current.event_head != changed.event_head
    {
        return Err(corrupt("Launch immutable fields changed"));
    }
    Ok(())
}

pub fn append_events(
    launch: &mut Launch,
    kinds: Vec<LaunchEventKind>,
    appended_at: String,
) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
    let mut appended = Vec::with_capacity(kinds.len());
    for kind in kinds {
        launch.event_head = launch
            .event_head
            .checked_add(1)
            .ok_or_else(|| corrupt("Launch event sequence overflow"))?;
        appended.push(LaunchEvent {
            launch_id: launch.id.clone(),
            seq: launch.event_head,
            appended_at: appended_at.clone(),
            kind,
        });
    }
    Ok(appended)
}

pub fn immutable_handoff(
    current: &ServiceConnectionHandoff,
    changed: &ServiceConnectionHandoff,
) -> Result<(), LaunchStoreError> {
    if current.id != changed.id
        || current.owner != changed.owner
        || current.launch_id != changed.launch_id
        || current.service_id != changed.service_id
        || current.intent_id != changed.intent_id
        || current.external_id != changed.external_id
    {
        return Err(corrupt("service handoff immutable fields changed"));
    }
    Ok(())
}
