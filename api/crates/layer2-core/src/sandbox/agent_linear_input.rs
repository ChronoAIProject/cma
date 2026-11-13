use std::collections::BTreeMap;

use super::{require_granted, AgentOperationService, MAX_OPERATION_COMMIT_RETRIES};
use crate::sandbox::{Agent, AgentOperationError, AgentOperationReceipt, AgentStatus};
use crate::{AgentId, Caller, RuntimeCapability};

impl AgentOperationService {
    /// Start a turn only while the exact predecessor still owns the conversation.
    pub(in crate::sandbox) async fn execute_linear_message(
        &self,
        caller: &Caller,
        id: &AgentId,
        text: String,
        previous_turn_sequence: Option<u64>,
        metadata: BTreeMap<String, String>,
    ) -> Result<AgentOperationReceipt, AgentOperationError> {
        self.require_owner_authority(caller)?;
        self.validate_message(&text)?;
        if !valid_metadata(&metadata) {
            return Err(AgentOperationError::InvalidRequest(
                "turn metadata is invalid".into(),
            ));
        }
        for _ in 0..MAX_OPERATION_COMMIT_RETRIES {
            let current = self.owned_agent(caller, id).await?;
            self.require_agent_connections(&current).await?;
            require_granted(&current, [RuntimeCapability::Conversation])?;
            if self.latest_turn(&current).await? != previous_turn_sequence {
                return Err(AgentOperationError::PreviousTurnConflict);
            }
            self.require_first_message_settled(&current)?;
            self.require_dispatch_available(&current)?;
            if current.status != AgentStatus::Ready {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            let (parent, queues) = self.parent_admission(&current).await?;
            if queues {
                return Err(AgentOperationError::StatusConflict {
                    status: current.status,
                });
            }
            match self
                .fence_message(current, text.clone(), parent, metadata.clone())
                .await
            {
                Ok(prepared) => return self.dispatch_prepared(prepared).await,
                Err(AgentOperationError::Conflict) => continue,
                Err(error) => return Err(error),
            }
        }
        Err(AgentOperationError::Conflict)
    }

    async fn latest_turn(&self, agent: &Agent) -> Result<Option<u64>, AgentOperationError> {
        self.store
            .agent_latest_started_turn(agent)
            .await
            .map_err(super::operation_store_error)
    }
}

pub(super) fn valid_metadata(metadata: &BTreeMap<String, String>) -> bool {
    metadata.len() <= 16
        && metadata
            .iter()
            .all(|(key, value)| key.chars().count() <= 64 && value.chars().count() <= 512)
}
