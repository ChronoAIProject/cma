use super::{AgentProvenanceSnapshot, Page, PageRequest, SandboxStoreError};
use crate::{sandbox::channel_usage::*, AgentId};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentChannelUsageSnapshot {
    pub task_status: crate::sandbox::AgentTaskStatus,
    pub provenance: AgentProvenanceSnapshot,
    pub state: AgentChannelUsageState,
}

impl AgentChannelUsageSnapshot {
    pub fn validate(&self, id: &AgentId) -> Result<(), SandboxStoreError> {
        self.state.validate()?;
        if self.state.creation != self.provenance.creation_provenance(id)? {
            return Err(SandboxStoreError::Corrupt(
                "channel usage creation identity changed".into(),
            ));
        }
        Ok(())
    }
}

/// These internal facts are never authorization evidence. Management services
/// must authorize reads and revalidate the included resource fence immediately
/// before disclosure. Mutation callers additionally need current bridge/grant
/// authority; this port only provides the Agent-side durable transaction.
#[async_trait::async_trait]
pub trait AgentChannelUsageStore: Send + Sync {
    async fn agent_channel_usage_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentChannelUsageSnapshot, SandboxStoreError>;

    /// One bounded append and summary update. Exact retries replay their original
    /// event after checking the current resource fence. No Agent, origin, log,
    /// runtime attachment, ownership or grant is rewritten.
    async fn record_agent_channel_usage(
        &self,
        request: ChannelUsageMutation,
    ) -> Result<ChannelUsageEvent, SandboxStoreError>;

    /// Stable ascending revision cursor. Includes a current authorization fence;
    /// every emitted and hidden lookahead event must pass integrity validation.
    async fn agent_channel_usage_history(
        &self,
        id: &AgentId,
        page: &PageRequest,
    ) -> Result<(AgentChannelUsageSnapshot, Page<ChannelUsageEvent>), SandboxStoreError>;
}

pub fn channel_usage_cursor(page: &PageRequest) -> Result<u64, SandboxStoreError> {
    PageRequest::new(page.limit, page.cursor.clone())?;
    match page.cursor.as_deref() {
        None => Ok(0),
        Some(value) if !value.is_empty() && value.bytes().all(|c| c.is_ascii_digit()) => value
            .parse::<u64>()
            .ok()
            .filter(|n| *n > 0 && *n <= i64::MAX as u64 && n.to_string() == value)
            .ok_or(SandboxStoreError::InvalidPage),
        Some(_) => Err(SandboxStoreError::InvalidPage),
    }
}

pub fn channel_usage_page(
    snapshot: &AgentChannelUsageSnapshot,
    page: &PageRequest,
    preceding: Option<&ChannelUsageEvent>,
    mut events: Vec<ChannelUsageEvent>,
) -> Result<Page<ChannelUsageEvent>, SandboxStoreError> {
    let cursor = channel_usage_cursor(page)?;
    snapshot.validate(&snapshot.state.creation.agent_id)?;
    let corrupt = || SandboxStoreError::Corrupt("channel usage history is incomplete".into());
    if cursor > snapshot.state.revision {
        return Err(SandboxStoreError::InvalidPage);
    }
    let mut previous = if cursor == 0 {
        AgentChannelUsageState::empty(snapshot.state.creation.clone())
    } else {
        let event = preceding.ok_or_else(corrupt)?;
        event.validate()?;
        if event.after.revision != cursor || event.after.creation != snapshot.state.creation {
            return Err(corrupt());
        }
        event.after.clone()
    };
    let expected = (snapshot.state.revision - cursor).min(u64::from(page.limit) + 1) as usize;
    if events.len() != expected {
        return Err(corrupt());
    }
    for event in &events {
        event.validate()?;
        if event.before != previous {
            return Err(corrupt());
        }
        previous = event.after.clone();
    }
    if previous.revision == snapshot.state.revision && previous != snapshot.state {
        return Err(corrupt());
    }
    let next = (events.len() > usize::from(page.limit)).then(|| {
        events[usize::from(page.limit) - 1]
            .after
            .revision
            .to_string()
    });
    events.truncate(usize::from(page.limit));
    Ok(Page {
        items: events,
        next,
    })
}
