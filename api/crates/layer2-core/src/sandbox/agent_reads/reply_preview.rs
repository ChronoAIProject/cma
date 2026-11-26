use super::*;
use crate::{resource_access::CallerContext, sandbox::AgentReplyPreview};

impl AgentReadService {
    pub async fn reply_preview(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentReplyPreview, AgentReadError> {
        let (agent, held) = if let Some(context) = context {
            if &context.caller != caller {
                return Err(AgentReadError::NotFound);
            }
            let held = self
                .authorized(context, credential, id, ResourceCapability::ViewHistory)
                .await?;
            let agent = held
                .snapshot
                .lineage
                .agent
                .clone()
                .ok_or_else(|| corrupt("reply preview Agent is absent"))?;
            require_granted(&agent, crate::RuntimeCapability::Conversation)?;
            (agent, Some(held))
        } else {
            let (lineage, held) = self
                .owner_projection_authority(caller, id, crate::RuntimeCapability::Conversation)
                .await?;
            (lineage.agent, held)
        };
        let result = self
            .store
            .agent_log_reply_preview(&agent)
            .await
            .map_err(store_error);
        self.finish_conversation_read(caller, &agent, held.as_ref())
            .await?;
        result
    }
}

#[cfg(test)]
#[path = "reply_preview/tests.rs"]
mod tests;
