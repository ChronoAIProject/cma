use async_trait::async_trait;

use super::*;
use crate::sandbox::store::AgentReadQueryStore;
use crate::sandbox::AgentListQuery;

#[async_trait]
impl AgentReadQueryStore for InMemorySandboxStore {
    async fn retained_agent_output(
        &self,
        agent: &Agent,
    ) -> Result<Option<AgentMessageCheckpoint>, SandboxStoreError> {
        let state = self.lock();
        let current = state
            .agents
            .get(&agent.id)
            .ok_or(SandboxStoreError::NotFound)?;
        if current.created_at != agent.created_at || current.owner != agent.owner {
            return Err(SandboxStoreError::NotFound);
        }
        Ok(state.agent_retained_output.get(&agent.id).cloned())
    }

    async fn list_owned_agents(
        &self,
        owner: &SubjectId,
        query: &AgentListQuery,
        request: &PageRequest,
    ) -> Result<Page<Agent>, SandboxStoreError> {
        let values = self
            .lock()
            .agents
            .values()
            .filter(|agent| &agent.owner == owner && query.matches(agent))
            .map(|agent| (agent.id.to_string(), agent.clone()))
            .collect();
        page(values, request)
    }
}

#[cfg(test)]
impl InMemorySandboxStore {
    pub(crate) fn retain_agent_output_for_test(
        &self,
        id: &AgentId,
        prefix: AgentMessageCheckpoint,
    ) {
        self.lock().agent_retained_output.insert(id.clone(), prefix);
    }

    pub(crate) fn replace_agent_log_for_test(&self, id: &AgentId, entries: Vec<AgentLogEntry>) {
        self.lock().agent_logs.insert(id.clone(), entries);
    }
}

#[cfg(test)]
impl InMemorySandboxStore {
    pub(crate) fn with_agent_fixture(agent: Agent) -> Self {
        agent
            .work
            .validate_resolved_skills(&agent.profile.content.skills)
            .expect("valid stored skill material");
        let store = Self::default();
        store.lock().agents.insert(agent.id.clone(), agent);
        store
    }
}
