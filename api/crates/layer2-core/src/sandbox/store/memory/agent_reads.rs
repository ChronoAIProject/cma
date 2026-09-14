use async_trait::async_trait;

use super::*;
use crate::sandbox::store::AgentReadQueryStore;
use crate::sandbox::AgentListQuery;

#[async_trait]
impl AgentReadQueryStore for InMemorySandboxStore {
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
    pub(crate) fn replace_agent_log_for_test(&self, id: &AgentId, entries: Vec<AgentLogEntry>) {
        self.lock().agent_logs.insert(id.clone(), entries);
    }
}
