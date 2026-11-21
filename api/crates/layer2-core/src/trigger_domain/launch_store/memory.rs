use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};

use async_trait::async_trait;

use crate::ids::{LaunchId, PublicTriggerId, ServiceHandoffId, SubjectId};

use super::*;
use crate::trigger_domain::{InMemoryTriggerStore, TriggerAvailability};

type CommandKey = (SubjectId, PublicTriggerId, String);
type HandoffCommandKey = (SubjectId, LaunchId, String);

#[derive(Clone)]
pub struct InMemoryLaunchStore {
    state: Arc<Mutex<State>>,
    triggers: Arc<InMemoryTriggerStore>,
}

#[derive(Default)]
pub(super) struct State {
    pub(super) launches: HashMap<LaunchId, Launch>,
    commands: HashMap<CommandKey, LaunchId>,
    events: HashMap<LaunchId, Vec<LaunchEvent>>,
    handoffs: HashMap<ServiceHandoffId, ServiceConnectionHandoff>,
    handoff_commands: HashMap<HandoffCommandKey, ServiceHandoffId>,
}

impl InMemoryLaunchStore {
    pub fn new(triggers: Arc<InMemoryTriggerStore>) -> Self {
        Self {
            state: Arc::new(Mutex::new(State::default())),
            triggers,
        }
    }

    fn lock(&self) -> MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

#[async_trait]
impl LaunchStore for InMemoryLaunchStore {
    async fn visitor_launches_admitted(
        &self,
        owner: &SubjectId,
        utc_day: u64,
    ) -> Result<u32, LaunchStoreError> {
        self.lock()
            .launches
            .values()
            .filter(|launch| {
                launch.owner == *owner
                    && launch.authority == super::super::LaunchAuthorityClass::Visitor
            })
            .try_fold(0_u32, |used, launch| {
                let timestamp = crate::seconds_from_rfc3339(&launch.created_at)
                    .ok_or_else(|| corrupt("invalid Launch time"))?;
                if super::super::visitor_launch_day(timestamp).0 == utc_day {
                    used.checked_add(1)
                        .ok_or_else(|| corrupt("visitor Launch count overflow"))
                } else {
                    Ok(used)
                }
            })
    }

    async fn admit_launch(
        &self,
        admission: LaunchAdmission,
    ) -> Result<LaunchAdmissionResult, LaunchStoreError> {
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
        validate_admission(&admission)?;
        if state.launches.contains_key(&admission.launch.id) {
            return Err(LaunchStoreError::Duplicate);
        }
        let mut launch = admission.launch;
        let event_at = launch.created_at.clone();
        let events = append_events(&mut launch, admission.events, event_at)?;
        launch.validate()?;
        let ready = launch.state == LaunchState::Ready;
        self.triggers
            .commit_launch_admission(&admission.trigger, ready, || {
                state.commands.insert(key, launch.id.clone());
                state.events.insert(launch.id.clone(), events);
                state.launches.insert(launch.id.clone(), launch.clone());
                LaunchAdmissionResult::Created(launch)
            })
    }

    async fn launch_by_command(
        &self,
        owner: &SubjectId,
        public_id: &PublicTriggerId,
        command_key_hash: &str,
    ) -> Result<Option<Launch>, LaunchStoreError> {
        let state = self.lock();
        let key = (
            owner.clone(),
            public_id.clone(),
            command_key_hash.to_string(),
        );
        let Some(id) = state.commands.get(&key) else {
            return Ok(None);
        };
        let launch = state
            .launches
            .get(id)
            .cloned()
            .ok_or_else(|| corrupt("launch command points to no aggregate"))?;
        if &launch.owner != owner
            || &launch.public_id != public_id
            || launch.command_key_hash != command_key_hash
        {
            return Err(corrupt("launch command points to a different aggregate"));
        }
        launch.validate()?;
        Ok(Some(launch))
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

    async fn list_launches(
        &self,
        owner: &SubjectId,
        page: &LaunchPageRequest,
    ) -> Result<LaunchPage, LaunchStoreError> {
        super::memory_queries::list(&self.lock(), owner, page)
    }

    async fn placement_stats(
        &self,
        trigger_id: &TriggerId,
    ) -> Result<Vec<LaunchPlacementStats>, LaunchStoreError> {
        super::memory_queries::placement_stats(&self.lock(), trigger_id)
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
        let became_ready = current.ready_at.is_none() && launch.ready_at.is_some();
        if became_ready {
            let trigger_id = launch.trigger_id.clone();
            return self.triggers.commit_agent_ready(&trigger_id, || {
                apply_launch_commit(&mut state, launch, appended)
            });
        }
        Ok(apply_launch_commit(&mut state, launch, appended))
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
            .filter(|launch| launch.is_worker_due(now))
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
        if !current.is_worker_due(&now) {
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
    async fn handoff_by_intent(
        &self,
        owner: &SubjectId,
        launch_id: &LaunchId,
        intent_id: &str,
    ) -> Result<Option<ServiceConnectionHandoff>, LaunchStoreError> {
        let state = self.lock();
        let key = (owner.clone(), launch_id.clone(), intent_id.to_owned());
        let Some(id) = state.handoff_commands.get(&key) else {
            return Ok(None);
        };
        let handoff = state
            .handoffs
            .get(id)
            .ok_or_else(|| corrupt("handoff command points to no record"))?;
        if &handoff.owner != owner
            || &handoff.launch_id != launch_id
            || handoff.intent_id != intent_id
        {
            return Err(corrupt("handoff command crossed its intent boundary"));
        }
        handoff.validate()?;
        Ok(Some(handoff.clone()))
    }

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
    if admission.trigger.revision.content.consent.always_review
        && admission.launch.started_at.is_none()
        && !admission.launch.confirmations.is_empty()
    {
        return Err(corrupt("publisher-required review cannot be preconfirmed"));
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

fn apply_launch_commit(state: &mut State, launch: Launch, appended: Vec<LaunchEvent>) -> Launch {
    state
        .events
        .entry(launch.id.clone())
        .or_default()
        .extend(appended);
    state.launches.insert(launch.id.clone(), launch.clone());
    launch
}

fn corrupt(detail: &str) -> LaunchStoreError {
    LaunchStoreError::Corrupt(detail.into())
}
