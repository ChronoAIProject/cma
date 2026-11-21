use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use async_trait::async_trait;

use crate::ids::{AgentProfileId, SandboxProfileId, SubjectId, WorkspaceProfileId};

use super::*;
use crate::trigger_domain::{LaunchStoreError, TriggerProfileResolver};

#[derive(Clone)]
pub struct InMemoryTriggerStore {
    profiles: Arc<dyn TriggerProfileResolver>,
    state: Arc<Mutex<State>>,
}

#[derive(Default)]
struct State {
    triggers: HashMap<TriggerId, Trigger>,
    public_ids: HashMap<PublicTriggerId, TriggerId>,
}

impl InMemoryTriggerStore {
    pub fn new(profiles: Arc<dyn TriggerProfileResolver>) -> Self {
        Self {
            profiles,
            state: Arc::new(Mutex::new(State::default())),
        }
    }

    fn lock(&self) -> std::sync::MutexGuard<'_, State> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn mutate_owned<T>(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        change: impl FnOnce(&mut Trigger) -> Result<T, TriggerStoreError>,
    ) -> Result<T, TriggerStoreError> {
        let mut state = self.lock();
        let stored = state
            .triggers
            .get(id)
            .filter(|trigger| &trigger.owner == owner && trigger.archived_at.is_none())
            .ok_or(TriggerStoreError::NotFound)?;
        let mut changed = stored.clone();
        let result = change(&mut changed)?;
        changed.validate()?;
        state.triggers.insert(id.clone(), changed);
        Ok(result)
    }

    fn public_trigger(state: &State, id: &PublicTriggerId) -> Option<PublicTrigger> {
        let trigger = state
            .public_ids
            .get(id)
            .and_then(|trigger_id| state.triggers.get(trigger_id))?;
        let revision = trigger.latest_revision()?.clone();
        Some(PublicTrigger {
            trigger_id: trigger.id.clone(),
            public_id: id.clone(),
            owner: trigger.owner.clone(),
            availability: trigger.availability,
            revision,
        })
    }

    fn guard_references(
        &self,
        references: impl Fn(&TriggerRevision) -> bool,
    ) -> Result<(), TriggerStoreError> {
        super::memory_guards::guard_references(&self.lock().triggers, references)
    }

    pub(crate) fn commit_launch_admission<T>(
        &self,
        expected: &PublicTrigger,
        ready: bool,
        commit: impl FnOnce() -> T,
    ) -> Result<T, LaunchStoreError> {
        let mut state = self.lock();
        let current = Self::public_trigger(&state, &expected.public_id)
            .ok_or(LaunchStoreError::TriggerUnavailable)?;
        if current.availability != TriggerAvailability::Published {
            return Err(LaunchStoreError::TriggerRetired);
        }
        if current != *expected {
            return Err(LaunchStoreError::TriggerUnavailable);
        }
        let stored = state
            .triggers
            .get(&current.trigger_id)
            .cloned()
            .ok_or(LaunchStoreError::TriggerUnavailable)?;
        let mut changed = stored;
        changed.record_launch()?;
        if ready {
            changed.record_agent_ready()?;
        }
        changed.validate()?;
        let result = commit();
        state.triggers.insert(changed.id.clone(), changed);
        Ok(result)
    }

    pub(crate) fn commit_agent_ready<T>(
        &self,
        id: &TriggerId,
        commit: impl FnOnce() -> T,
    ) -> Result<T, LaunchStoreError> {
        let mut state = self.lock();
        let stored = state
            .triggers
            .get(id)
            .cloned()
            .ok_or(LaunchStoreError::TriggerUnavailable)?;
        let mut changed = stored;
        changed.record_agent_ready()?;
        changed.validate()?;
        let result = commit();
        state.triggers.insert(id.clone(), changed);
        Ok(result)
    }

    #[cfg(test)]
    pub(crate) fn seed_trigger(&self, trigger: Trigger) -> Result<(), TriggerStoreError> {
        trigger.validate()?;
        let public_id = trigger
            .public_id
            .clone()
            .ok_or_else(|| TriggerError::new("invalid_trigger_state", "public_id"))?;
        let mut state = self.lock();
        if state.triggers.contains_key(&trigger.id) || state.public_ids.contains_key(&public_id) {
            return Err(TriggerStoreError::Duplicate);
        }
        state.public_ids.insert(public_id, trigger.id.clone());
        state.triggers.insert(trigger.id.clone(), trigger);
        Ok(())
    }
}

#[async_trait]
impl TriggerStore for InMemoryTriggerStore {
    async fn create_trigger(&self, trigger: Trigger) -> Result<(), TriggerStoreError> {
        trigger.validate()?;
        if trigger.availability != TriggerAvailability::Draft
            || trigger.public_id.is_some()
            || !trigger.revisions.is_empty()
        {
            return Err(TriggerError::new("invalid_trigger_state", "availability").into());
        }
        let mut state = self.lock();
        if state.triggers.contains_key(&trigger.id) {
            return Err(TriggerStoreError::Duplicate);
        }
        state.triggers.insert(trigger.id.clone(), trigger);
        Ok(())
    }

    async fn trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
    ) -> Result<Option<Trigger>, TriggerStoreError> {
        Ok(self
            .lock()
            .triggers
            .get(id)
            .filter(|trigger| &trigger.owner == owner && trigger.archived_at.is_none())
            .cloned())
    }

    async fn delete_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        archived_at: String,
    ) -> Result<(), TriggerStoreError> {
        let mut state = self.lock();
        let trigger = state
            .triggers
            .get(id)
            .filter(|trigger| &trigger.owner == owner && trigger.archived_at.is_none())
            .ok_or(TriggerStoreError::NotFound)?;
        trigger
            .validate()
            .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))?;
        if trigger.availability != TriggerAvailability::Draft
            || trigger.public_id.is_some()
            || !trigger.revisions.is_empty()
        {
            let mut archived = trigger.clone();
            archived.archive(archived_at)?;
            archived.validate()?;
            state.triggers.insert(id.clone(), archived);
        } else {
            state.triggers.remove(id);
        }
        Ok(())
    }

    async fn trigger_revision(
        &self,
        id: &TriggerId,
        digest: &str,
    ) -> Result<Option<TriggerRevision>, TriggerStoreError> {
        let state = self.lock();
        let Some(trigger) = state.triggers.get(id) else {
            return Ok(None);
        };
        if &trigger.id != id {
            return Err(TriggerStoreError::Corrupt(
                "Trigger store key disagrees with its aggregate".into(),
            ));
        }
        let Some(revision) = trigger
            .revisions
            .iter()
            .find(|revision| revision.digest == digest)
        else {
            return Ok(None);
        };
        revision
            .validate()
            .map_err(|error| TriggerStoreError::Corrupt(error.to_string()))?;
        Ok(Some(revision.clone()))
    }

    async fn save_draft(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        expected: u64,
        draft: super::super::TriggerContent,
        updated_at: String,
    ) -> Result<Trigger, TriggerStoreError> {
        self.mutate_owned(id, owner, |trigger| {
            if trigger.draft_version != expected {
                return Err(TriggerStoreError::Conflict {
                    expected,
                    actual: trigger.draft_version,
                });
            }
            trigger.replace_draft(expected, draft, updated_at)?;
            Ok(trigger.clone())
        })
    }

    async fn publish_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        expected: u64,
        published_at: String,
    ) -> Result<PublicTrigger, TriggerStoreError> {
        let before = self
            .trigger(id, owner)
            .await?
            .ok_or(TriggerStoreError::NotFound)?;
        if before.draft_version != expected {
            return Err(TriggerStoreError::Conflict {
                expected,
                actual: before.draft_version,
            });
        }
        self.profiles
            .resolve_trigger_profiles(owner, &before.draft.target)
            .await?;

        let mut state = self.lock();
        let stored = state
            .triggers
            .get(id)
            .filter(|trigger| &trigger.owner == owner && trigger.archived_at.is_none())
            .ok_or(TriggerStoreError::NotFound)?;
        if stored.draft_version != expected || stored.draft != before.draft {
            return Err(TriggerStoreError::Conflict {
                expected,
                actual: stored.draft_version,
            });
        }
        let mut changed = stored.clone();
        let revision = changed.publish(published_at)?;
        let public_id = changed
            .public_id
            .clone()
            .ok_or_else(|| TriggerError::new("invalid_trigger_state", "public_id"))?;
        if state
            .public_ids
            .get(&public_id)
            .is_some_and(|mapped| mapped != id)
        {
            return Err(TriggerStoreError::Duplicate);
        }
        changed.validate()?;
        state.public_ids.insert(public_id.clone(), id.clone());
        state.triggers.insert(id.clone(), changed);
        Ok(PublicTrigger {
            trigger_id: id.clone(),
            public_id,
            owner: owner.clone(),
            availability: TriggerAvailability::Published,
            revision,
        })
    }

    async fn trigger_by_public_id(
        &self,
        id: &PublicTriggerId,
    ) -> Result<Option<PublicTrigger>, TriggerStoreError> {
        Ok(Self::public_trigger(&self.lock(), id))
    }

    async fn preview_by_public_id(
        &self,
        id: &PublicTriggerId,
        publisher_display_name: String,
    ) -> Result<Option<TriggerPreview>, TriggerStoreError> {
        let Some(published) = self.trigger_by_public_id(id).await? else {
            return Ok(None);
        };
        let profiles = self
            .profiles
            .read_trigger_profiles(&published.revision.content.target)
            .await?;
        Ok(Some(TriggerPreview::project(
            published.availability,
            &published.revision,
            publisher_display_name,
            &profiles,
        )?))
    }

    async fn list_own_triggers(
        &self,
        owner: &SubjectId,
        page: &TriggerPageRequest,
    ) -> Result<TriggerPage<Trigger>, TriggerStoreError> {
        page.validate()?;
        let mut triggers: Vec<_> = self
            .lock()
            .triggers
            .values()
            .filter(|trigger| &trigger.owner == owner && trigger.archived_at.is_none())
            .cloned()
            .collect();
        triggers.sort_by(|left, right| left.id.cmp(&right.id));
        if let Some(cursor) = &page.cursor {
            triggers.retain(|trigger| trigger.id.as_str() > cursor.as_str());
        }
        let has_more = triggers.len() > usize::from(page.limit);
        triggers.truncate(usize::from(page.limit));
        let next = has_more
            .then(|| triggers.last().map(|trigger| trigger.id.to_string()))
            .flatten();
        Ok(TriggerPage {
            items: triggers,
            next,
        })
    }

    async fn retire_trigger(
        &self,
        id: &TriggerId,
        owner: &SubjectId,
        retired_at: String,
    ) -> Result<Trigger, TriggerStoreError> {
        self.mutate_owned(id, owner, |trigger| {
            trigger.retire(retired_at)?;
            Ok(trigger.clone())
        })
    }

    async fn create_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        source_url: String,
        label: String,
    ) -> Result<Placement, TriggerStoreError> {
        let placement = Placement::new(source_url, label)?;
        self.mutate_owned(trigger_id, owner, |trigger| {
            trigger.add_placement(placement.clone())?;
            Ok(placement)
        })
    }

    async fn placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
    ) -> Result<Option<Placement>, TriggerStoreError> {
        Ok(self.trigger(trigger_id, owner).await?.and_then(|trigger| {
            trigger
                .placements
                .into_iter()
                .find(|placement| &placement.id == placement_id)
        }))
    }

    async fn list_placements(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        include_retired: bool,
    ) -> Result<Vec<Placement>, TriggerStoreError> {
        let trigger = self
            .trigger(trigger_id, owner)
            .await?
            .ok_or(TriggerStoreError::NotFound)?;
        Ok(trigger
            .placements
            .into_iter()
            .filter(|placement| include_retired || placement.retired_at.is_none())
            .collect())
    }

    async fn update_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        update: PlacementUpdate,
    ) -> Result<Placement, TriggerStoreError> {
        self.mutate_owned(trigger_id, owner, |trigger| {
            Ok(trigger.update_placement(placement_id, update)?)
        })
    }

    async fn retire_placement(
        &self,
        trigger_id: &TriggerId,
        owner: &SubjectId,
        placement_id: &PlacementId,
        retired_at: String,
    ) -> Result<Placement, TriggerStoreError> {
        self.mutate_owned(trigger_id, owner, |trigger| {
            Ok(trigger.retire_placement(placement_id, retired_at)?)
        })
    }

    async fn record_launch(&self, id: &TriggerId) -> Result<TriggerStats, TriggerStoreError> {
        let owner = self
            .lock()
            .triggers
            .get(id)
            .map(|trigger| trigger.owner.clone())
            .ok_or(TriggerStoreError::NotFound)?;
        self.mutate_owned(id, &owner, |trigger| {
            trigger.record_launch()?;
            Ok(trigger.stats.clone())
        })
    }

    async fn record_agent_ready(&self, id: &TriggerId) -> Result<TriggerStats, TriggerStoreError> {
        let mut state = self.lock();
        let mut trigger = state
            .triggers
            .get(id)
            .cloned()
            .ok_or(TriggerStoreError::NotFound)?;
        trigger.record_agent_ready()?;
        trigger.validate()?;
        let stats = trigger.stats.clone();
        state.triggers.insert(id.clone(), trigger);
        Ok(stats)
    }

    async fn guard_sandbox_profile_retirement(
        &self,
        id: &SandboxProfileId,
    ) -> Result<(), TriggerStoreError> {
        self.guard_references(|revision| &revision.content.target.sandbox_profile.id == id)
    }

    async fn guard_workspace_profile_retirement(
        &self,
        id: &WorkspaceProfileId,
    ) -> Result<(), TriggerStoreError> {
        self.guard_references(|revision| &revision.content.target.workspace_profile.id == id)
    }

    async fn guard_agent_profile_retirement(
        &self,
        id: &AgentProfileId,
    ) -> Result<(), TriggerStoreError> {
        self.guard_references(|revision| &revision.content.target.agent_profile.id == id)
    }
}
