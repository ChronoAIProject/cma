use std::collections::BTreeMap;

use super::{require_granted, AgentOperationService, MAX_OPERATION_COMMIT_RETRIES};
use crate::sandbox::store::{AgentLogStore, MAX_PAGE_SIZE};
use crate::sandbox::{
    Agent, AgentLogKind, AgentOperationError, AgentOperationReceipt, AgentStatus,
};
use crate::{AgentId, Caller, RuntimeCapability};

impl AgentOperationService {
    /// Start a turn only while the exact predecessor still owns the conversation.
    pub async fn send_linear_message(
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
        let mut after = 0;
        let mut latest = None;
        while after < agent.conversation.last_seq {
            let limit = (agent.conversation.last_seq - after).min(u64::from(MAX_PAGE_SIZE)) as u16;
            let entries = AgentLogStore::agent_log(self.store.as_ref(), &agent.id, after, limit)
                .await
                .map_err(super::operation_store_error)?;
            if entries.len() != usize::from(limit) {
                return Err(AgentOperationError::Corrupt(
                    "agent log ended before its high-water mark".into(),
                ));
            }
            for entry in entries {
                after += 1;
                if entry.agent_id != agent.id || entry.seq != after {
                    return Err(AgentOperationError::Corrupt(
                        "agent log sequence is invalid".into(),
                    ));
                }
                if let AgentLogKind::TurnStarted { turn_sequence, .. } = entry.kind {
                    if turn_sequence != entry.seq {
                        return Err(AgentOperationError::Corrupt(
                            "agent turn sequence is invalid".into(),
                        ));
                    }
                    latest = Some(turn_sequence);
                }
            }
        }
        Ok(latest)
    }
}

pub(super) fn valid_metadata(metadata: &BTreeMap<String, String>) -> bool {
    metadata.len() <= 16
        && metadata
            .iter()
            .all(|(key, value)| key.chars().count() <= 64 && value.chars().count() <= 512)
}
