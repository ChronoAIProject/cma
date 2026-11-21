use super::*;
use crate::sandbox::{
    channel_usage::*,
    store::{
        channel_usage_cursor, channel_usage_page, AgentChannelUsageSnapshot, AgentChannelUsageStore,
    },
};

pub(super) fn snapshot(
    state: &State,
    id: &AgentId,
) -> Result<AgentChannelUsageSnapshot, SandboxStoreError> {
    let provenance = super::provenance::snapshot(state, id)?;
    let latest = state
        .channel_usage_events
        .range((id.clone(), 0)..=(id.clone(), u64::MAX))
        .next_back();
    let usage = match state.agent_channel_usage.get(id) {
        Some(stored) => {
            let (key, latest) = latest.ok_or_else(|| {
                SandboxStoreError::Corrupt("channel usage latest event missing".into())
            })?;
            if key.1 != stored.state.revision {
                return Err(SandboxStoreError::Corrupt(
                    "channel usage latest event index changed".into(),
                ));
            }
            stored.validate_latest(latest)?;
            stored.state.clone()
        }
        None => {
            if latest.is_some() {
                return Err(SandboxStoreError::Corrupt(
                    "channel usage summary missing".into(),
                ));
            }
            AgentChannelUsageState::empty(provenance.creation_provenance(id)?)
        }
    };
    let snapshot = AgentChannelUsageSnapshot {
        task_status: Default::default(),
        provenance,
        state: usage,
    };
    snapshot.validate(id)?;
    Ok(snapshot)
}

#[async_trait::async_trait]
impl AgentChannelUsageStore for InMemorySandboxStore {
    async fn agent_channel_usage_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentChannelUsageSnapshot, SandboxStoreError> {
        snapshot(&self.lock(), id)
    }

    async fn record_agent_channel_usage(
        &self,
        request: ChannelUsageMutation,
    ) -> Result<ChannelUsageEvent, SandboxStoreError> {
        request.validate()?;
        let mut state = self.lock();
        let current = snapshot(&state, &request.creation.agent_id)?;
        request.require_current(&current.provenance)?;
        if let Some(key) = state.channel_usage_operations.get(&request.operation_id()) {
            let event = state.channel_usage_events.get(key).ok_or_else(|| {
                SandboxStoreError::Corrupt("channel usage receipt missing".into())
            })?;
            return request.replay(event.clone());
        }
        let binding = request.change.introduced_binding();
        if binding.is_some_and(|b| state.channel_usage_bindings.contains_key(&b.binding_id)) {
            return Err(SandboxStoreError::Conflict);
        }
        let predecessor = binding
            .and_then(|b| b.previous_binding_id.as_ref())
            .map(|id| {
                let key = state
                    .channel_usage_bindings
                    .get(id)
                    .ok_or(SandboxStoreError::Conflict)?;
                let event = state.channel_usage_events.get(key).ok_or_else(|| {
                    SandboxStoreError::Corrupt("channel binding history missing".into())
                })?;
                event.validate()?;
                event
                    .change
                    .introduced_binding()
                    .filter(|b| &b.binding_id == id)
                    .cloned()
                    .ok_or_else(|| {
                        SandboxStoreError::Corrupt("channel binding index changed".into())
                    })
            })
            .transpose()?;
        let event = plan_channel_usage(&request, &current.provenance, &current.state, predecessor)?;
        let key = (request.creation.agent_id.clone(), event.after.revision);
        if state.channel_usage_events.contains_key(&key) {
            return Err(SandboxStoreError::SequenceConflict);
        }
        state.agent_channel_usage.insert(
            request.creation.agent_id,
            StoredAgentChannelUsage {
                state: event.after.clone(),
                latest_event_digest: event.digest(),
            },
        );
        state
            .channel_usage_operations
            .insert(event.operation_id.clone(), key.clone());
        if let Some(binding) = binding {
            state
                .channel_usage_bindings
                .insert(binding.binding_id.clone(), key.clone());
        }
        state.channel_usage_events.insert(key, event.clone());
        Ok(event)
    }

    async fn agent_channel_usage_history(
        &self,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<(AgentChannelUsageSnapshot, Page<ChannelUsageEvent>), SandboxStoreError> {
        let cursor = channel_usage_cursor(page)?;
        let state = self.lock();
        let snapshot = snapshot(&state, id)?;
        let preceding = state.channel_usage_events.get(&(id.clone(), cursor));
        let events = state
            .channel_usage_events
            .range((id.clone(), cursor.saturating_add(1))..)
            .take_while(|((agent, _), _)| agent == id)
            .take(usize::from(page.limit) + 1)
            .map(|(_, event)| event.clone())
            .collect();
        let page = channel_usage_page(&snapshot, page, preceding, events)?;
        Ok((snapshot, page))
    }
}
