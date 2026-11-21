use super::agent_operation_dispatch::{operation_log, runtime_target};
use super::agent_operation_service::{operation_store_error, MAX_OPERATION_COMMIT_RETRIES};
use super::{
    AgentOperationError, AgentOperationIntent, AgentOperationService, AgentRuntimeTarget,
    AgentSettings, AgentWorkAuthority,
};
use crate::now_rfc3339;

impl AgentOperationService {
    /// Record acknowledged absolute settings replay under the current worker
    /// lease. User input remains fenced until this durable confirmation commits.
    pub async fn confirm_settings_replayed(
        &self,
        target: &AgentRuntimeTarget,
        authority: &AgentWorkAuthority,
        settings: &AgentSettings,
    ) -> Result<(), AgentOperationError> {
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self
                .store
                .get_agent(&target.agent_id)
                .await
                .map_err(operation_store_error)?
                .ok_or(AgentOperationError::NotFound)?;
            if !current.work.owns(&authority.holder, authority.fence)
                || runtime_target(&current)? != *target
                || current.conversation.settings.as_ref() != Some(settings)
            {
                return Err(AgentOperationError::StaleWorkAuthority);
            }
            let Some(operation) = current.conversation.operation.as_ref().filter(|operation| {
                matches!(
                    operation.intent,
                    AgentOperationIntent::UpdateSettings { .. }
                ) && operation.blocks_dispatch()
            }) else {
                return Ok(());
            };
            let sequence = operation.sequence;
            let at = now_rfc3339();
            if current
                .work
                .lease
                .as_ref()
                .is_none_or(|lease| lease.expires_at <= at)
            {
                return Err(AgentOperationError::StaleWorkAuthority);
            }
            let mut next = current.clone();
            next.conversation
                .operation
                .as_mut()
                .expect("checked")
                .mark_execution_observed(at.clone())
                .map_err(|error| AgentOperationError::Corrupt(error.to_string()))?;
            let logs = vec![operation_log(&next, sequence)?];
            match self.commit_change(&current, next, logs, at).await {
                Ok(_) => return Ok(()),
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }
}
