use std::collections::{BTreeMap, HashMap};
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use crate::ids::{LaunchId, PublicTriggerId, ServiceHandoffId, SubjectId, TriggerId};

use super::*;
use crate::trigger_domain::TriggerAvailability;

type CommandKey = (SubjectId, PublicTriggerId, String);
type HandoffCommandKey = (SubjectId, LaunchId, String);

#[derive(Clone, Default)]
pub struct InMemoryLaunchStore {
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    launches: HashMap<LaunchId, Launch>,
    commands: HashMap<CommandKey, LaunchId>,
    events: HashMap<LaunchId, Vec<LaunchEvent>>,
    handoffs: HashMap<ServiceHandoffId, ServiceConnectionHandoff>,
    handoff_commands: HashMap<HandoffCommandKey, ServiceHandoffId>,
    trigger_launches: BTreeMap<TriggerId, u64>,
}

impl InMemoryLaunchStore {
    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    #[cfg(test)]
    pub(crate) fn trigger_launch_count(&self, id: &TriggerId) -> u64 {
        self.lock().trigger_launches.get(id).copied().unwrap_or(0)
    }
}

#[async_trait]
impl LaunchStore for InMemoryLaunchStore {
    async fn admit_launch(
        &self,
        admission: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError> {
        validate_admission(&admission)?;
        let mut state = self.lock();
        let key = (
            admission.launch.owner.clone(),
            admission.launch.public_id.clone(),
            admission.launch.command_key_hash.clone(),
        );
        if let Some(id) = state.commands.get(&key) {
            let stored = state
                .launches
                .get(id)
                .ok_or_else(|| corrupt("launch command points to no aggregate"))?;
            if stored.command_fingerprint != admission.launch.command_fingerprint {
                return Err(LaunchStoreError::CommandConflict);
            }
            return Ok(LaunchAdmissionResult::Existing(stored.clone()));
        }
        if state.launches.contains_key(&admission.launch.id) {
            return Err(LaunchStoreError::Duplicate);
        }
        let mut launch = admission.launch;
        let event_at = launch.created_at.clone();
        let events = append_events(&mut launch, admission.events, event_at)?;
        launch.validate()?;
        state.commands.insert(key, launch.id.clone());
        state.events.insert(launch.id.clone(), events);
        state.launches.insert(launch.id.clone(), launch.clone());
        let count = state
            .trigger_launches
            .entry(launch.trigger_id.clone())
            .or_default();
        *count = count
            .checked_add(1)
            .ok_or_else(|| corrupt("trigger launch count overflow"))?;
        Ok(LaunchAdmissionResult::Created(launch))
    }

    async fn launch(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        Ok(self
            .lock()
            .launches
            .get(id)
            .filter(|launch| &launch.owner == owner)
            .cloned())
    }

    async fn commit_launch(&self, commit: LaunchCommit) -> Result<Launch, LaunchStoreError> {
        let mut state = self.lock();
        let current = state
            .launches
            .get(&commit.launch.id)
            .cloned()
            .ok_or(LaunchStoreError::NotFound)?;
        if current.revision != commit.expected_revision {
            return Err(LaunchStoreError::Conflict {
                expected: commit.expected_revision,
                actual: current.revision,
            });
        }
        validate_immutable(&current, &commit.launch)?;
        if commit.launch.revision != current.revision.saturating_add(1) {
            return Err(corrupt(
                "launch commit did not advance exactly one revision",
            ));
        }
        let mut launch = commit.launch;
        let appended = append_events(&mut launch, commit.events, commit.event_at)?;
        launch.validate()?;
        state
            .events
            .entry(launch.id.clone())
            .or_default()
            .extend(appended);
        state.launches.insert(launch.id.clone(), launch.clone());
        Ok(launch)
    }

    async fn launch_events(
        &self,
        id: &LaunchId,
        owner: &SubjectId,
        after: u64,
        limit: u16,
    ) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
        if limit == 0 || limit > MAX_LAUNCH_EVENT_LIMIT {
            return Err(LaunchStoreError::InvalidCursor);
        }
        let state = self.lock();
        let launch = state
            .launches
            .get(id)
            .filter(|launch| &launch.owner == owner)
            .ok_or(LaunchStoreError::NotFound)?;
        if after > launch.event_head {
            return Err(LaunchStoreError::InvalidCursor);
        }
        Ok(state
            .events
            .get(id)
            .into_iter()
            .flatten()
            .filter(|event| event.seq > after)
            .take(usize::from(limit))
            .cloned()
            .collect())
    }

    async fn due_launches(&self, now: &str, limit: u16) -> Result<Vec<LaunchId>, LaunchStoreError> {
        if limit == 0 || limit > MAX_LAUNCH_EVENT_LIMIT {
            return Err(LaunchStoreError::InvalidCursor);
        }
        let mut due = self
            .lock()
            .launches
            .values()
            .filter(|launch| launch.blocked_reason.is_none() && launch.work.is_due(now))
            .map(|launch| (launch.work.ready_at.clone(), launch.id.clone()))
            .collect::<Vec<_>>();
        due.sort();
        due.truncate(usize::from(limit));
        Ok(due.into_iter().map(|(_, id)| id).collect())
    }

    async fn claim_launch(
        &self,
        id: &LaunchId,
        holder: &str,
        now: String,
        lease_expires_at: String,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        let mut state = self.lock();
        let Some(current) = state.launches.get(id).cloned() else {
            return Ok(None);
        };
        if current.blocked_reason.is_some() || !current.work.is_due(&now) {
            return Ok(None);
        }
        let mut claimed = current.clone();
        claimed
            .work
            .claim(holder.to_string(), &now, lease_expires_at)
            .map_err(|_| LaunchStoreError::LeaseHeld)?;
        claimed.revision = claimed
            .revision
            .checked_add(1)
            .ok_or_else(|| corrupt("launch revision overflow"))?;
        claimed.updated_at = now;
        claimed.validate()?;
        state.launches.insert(id.clone(), claimed.clone());
        Ok(Some(claimed))
    }
}

#[async_trait]
impl ServiceConnectionStore for InMemoryLaunchStore {
    async fn create_handoff(
        &self,
        handoff: ServiceConnectionHandoff,
    ) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
        handoff.validate()?;
        let mut state = self.lock();
        let launch = state
            .launches
            .get(&handoff.launch_id)
            .filter(|launch| launch.owner == handoff.owner)
            .ok_or(LaunchStoreError::NotFound)?;
        if launch.state.is_terminal() {
            return Err(LaunchStoreError::NotFound);
        }
        let key = (
            handoff.owner.clone(),
            handoff.launch_id.clone(),
            handoff.intent_id.clone(),
        );
        if let Some(id) = state.handoff_commands.get(&key) {
            let existing = state
                .handoffs
                .get(id)
                .ok_or_else(|| corrupt("handoff command points to no record"))?;
            if existing.service_id != handoff.service_id {
                return Err(LaunchStoreError::CommandConflict);
            }
            return Ok(existing.clone());
        }
        if state.handoffs.contains_key(&handoff.id) {
            return Err(LaunchStoreError::Duplicate);
        }
        state.handoff_commands.insert(key, handoff.id.clone());
        state.handoffs.insert(handoff.id.clone(), handoff.clone());
        Ok(handoff)
    }

    async fn handoff(
        &self,
        id: &ServiceHandoffId,
        owner: &SubjectId,
    ) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
        Ok(self
            .lock()
            .handoffs
            .get(id)
            .filter(|handoff| &handoff.owner == owner)
            .cloned())
    }

    async fn save_handoff(
        &self,
        expected_revision: u64,
        handoff: ServiceConnectionHandoff,
    ) -> Result<ServiceConnectionHandoff, LaunchStoreError> {
        handoff.validate()?;
        let mut state = self.lock();
        let current = state
            .handoffs
            .get(&handoff.id)
            .ok_or(LaunchStoreError::NotFound)?;
        if current.revision != expected_revision {
            return Err(LaunchStoreError::Conflict {
                expected: expected_revision,
                actual: current.revision,
            });
        }
        if handoff.revision != expected_revision.saturating_add(1)
            || handoff.owner != current.owner
            || handoff.launch_id != current.launch_id
            || handoff.service_id != current.service_id
            || handoff.intent_id != current.intent_id
            || handoff.external_id != current.external_id
        {
            return Err(corrupt("service handoff immutable fields changed"));
        }
        state.handoffs.insert(handoff.id.clone(), handoff.clone());
        Ok(handoff)
    }
}

fn validate_admission(admission: &LaunchAdmission) -> Result<(), LaunchStoreError> {
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
    if admission.events.is_empty()
        || !matches!(admission.events.first(), Some(LaunchEventKind::Created))
    {
        return Err(corrupt("launch admission omitted its Created event"));
    }
    Ok(())
}

fn validate_immutable(current: &Launch, changed: &Launch) -> Result<(), LaunchStoreError> {
    if current.id != changed.id
        || current.trigger_id != changed.trigger_id
        || current.revision_digest != changed.revision_digest
        || current.public_id != changed.public_id
        || current.owner != changed.owner
        || current.placement_id != changed.placement_id
        || current.referrer_origin != changed.referrer_origin
        || current.command_key_hash != changed.command_key_hash
        || current.command_fingerprint != changed.command_fingerprint
        || current.created_at != changed.created_at
        || current.event_head != changed.event_head
    {
        return Err(corrupt("launch immutable fields changed"));
    }
    Ok(())
}

fn append_events(
    launch: &mut Launch,
    kinds: Vec<LaunchEventKind>,
    at: String,
) -> Result<Vec<LaunchEvent>, LaunchStoreError> {
    let mut appended = Vec::with_capacity(kinds.len());
    for kind in kinds {
        launch.event_head = launch
            .event_head
            .checked_add(1)
            .ok_or_else(|| corrupt("launch event sequence overflow"))?;
        appended.push(LaunchEvent {
            launch_id: launch.id.clone(),
            seq: launch.event_head,
            appended_at: at.clone(),
            kind,
        });
    }
    Ok(appended)
}

fn corrupt(detail: &str) -> LaunchStoreError {
    LaunchStoreError::Corrupt(detail.into())
}
