use super::*;

impl AgentOperationService {
    pub(super) async fn refuse_first_message_budget(
        &self,
        current: &Agent,
        authority: &AgentWorkAuthority,
        reason: crate::budget::BudgetError,
    ) -> Result<FirstMessageDispatchOutcome, AgentOperationError> {
        require_work_authority(current, authority)?;
        let at = now_rfc3339();
        let mut next = current.clone();
        let record = next
            .conversation
            .first_message
            .as_mut()
            .ok_or_else(|| AgentOperationError::Corrupt("first message disappeared".into()))?;
        record
            .refuse_budget(reason, at.clone())
            .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
        let refused = record.clone();
        self.commit_change(current, next, vec![first_message_log(&refused)], at)
            .await?;
        Ok(FirstMessageDispatchOutcome::Accepted(refused))
    }
}
