use super::*;
use crate::resource_access::{ResourceAccessError, ResourceAccessStoreError};
use crate::sandbox::agent_artifacts::*;

#[async_trait]
impl AgentArtifactStore for InMemorySandboxStore {
    async fn agent_artifact_metadata(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifactMetadata>, ResourceAccessStoreError> {
        Ok(self
            .lock()
            .agent_artifacts
            .get(id)
            .filter(|row| &row.binding == binding)
            .map(StoredAgentArtifact::metadata_record))
    }

    async fn publish_artifact(
        &self,
        request: &ArtifactPublication,
    ) -> Result<StoredAgentArtifact, ResourceAccessError> {
        let mut state = self.lock();
        let snapshot = super::resource_access::snapshot(&state, &request.target())?;
        request.validate_snapshot(&snapshot)?;
        if let Some(old) = state.agent_artifacts.get(&request.record().metadata.id) {
            return request.replay(old.clone());
        }
        let count = state
            .agent_artifacts
            .values()
            .filter(|value| value.binding.agent_id == request.record().binding.agent_id)
            .count();
        if count >= MAX_AGENT_ARTIFACTS {
            return Err(ResourceAccessStoreError::Capacity.into());
        }
        request.record().content()?;
        request.require_current()?;
        state.agent_artifacts.insert(
            request.record().metadata.id.clone(),
            request.record().clone(),
        );
        Ok(request.record().clone())
    }
    async fn agent_artifacts(
        &self,
        binding: &ArtifactBinding,
        page: &ArtifactPageRequest,
    ) -> Result<Vec<StoredAgentArtifact>, ResourceAccessStoreError> {
        page.validate()?;
        let state = self.lock();
        let mut rows: Vec<_> = state
            .agent_artifacts
            .values()
            .filter(|row| {
                &row.binding == binding
                    && page
                        .cursor
                        .as_ref()
                        .is_none_or(|cursor| row.metadata.id > *cursor)
            })
            .cloned()
            .collect();
        rows.sort_by(|a, b| a.metadata.id.cmp(&b.metadata.id));
        rows.truncate(usize::from(page.limit) + 1);
        Ok(rows)
    }
    async fn agent_artifact(
        &self,
        binding: &ArtifactBinding,
        id: &str,
    ) -> Result<Option<StoredAgentArtifact>, ResourceAccessStoreError> {
        #[cfg(test)]
        self.artifact_body_reads
            .fetch_add(1, std::sync::atomic::Ordering::SeqCst);
        Ok(self
            .lock()
            .agent_artifacts
            .get(id)
            .filter(|row| &row.binding == binding)
            .cloned())
    }
}
