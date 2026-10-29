use super::*;

impl AgentReadService {
    /// Project an explicitly admitted recovery for its owner or an audited operator.
    /// Ordinary reads retain their existing personal-resource authorization.
    pub async fn recovery_lineage(
        &self,
        caller: &Caller,
        id: &AgentId,
    ) -> Result<AgentLineage, AgentReadError> {
        if !caller.authorization.allows(Capability::ControlAnySandbox) {
            return self.lineage(caller, id).await;
        }
        let agent = self
            .store
            .get_agent(id)
            .await
            .map_err(store_error)?
            .filter(|agent| agent.id == *id)
            .ok_or(AgentReadError::NotFound)?;
        let (worktree, workspace, sandbox) = self.validate_lineage(&agent.owner, &agent).await?;
        Ok(AgentLineage {
            agent,
            worktree,
            workspace,
            sandbox,
        })
    }
}
