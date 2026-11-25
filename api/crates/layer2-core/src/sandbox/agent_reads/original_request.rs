use super::*;
use crate::resource_access::CallerContext;
use crate::sandbox::FirstMessagePurpose;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentOriginalRequest {
    pub text: String,
    pub turn_sequence: Option<u64>,
}

impl AgentReadService {
    pub async fn original_request(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<Option<AgentOriginalRequest>, AgentReadError> {
        let (agent, held) = if let Some(context) = context {
            if &context.caller != caller {
                return Err(AgentReadError::NotFound);
            }
            let held = self
                .authorized(context, credential, id, ResourceCapability::ViewHistory)
                .await?;
            if !authorized::is_personal(&held, context) {
                return Err(AgentReadError::NotFound);
            }
            let agent = held
                .snapshot
                .lineage
                .agent
                .clone()
                .ok_or_else(|| corrupt("original request has no authoritative Agent"))?;
            require_granted(&agent, crate::RuntimeCapability::Conversation)?;
            (agent, Some(held))
        } else {
            let (lineage, held) = self
                .owner_projection_authority(caller, id, crate::RuntimeCapability::Conversation)
                .await?;
            (lineage.agent, held)
        };
        let result = project(agent.conversation.first_message.as_ref());
        let current = self
            .finish_conversation_read(caller, &agent, held.as_ref())
            .await?;
        if request_identity(&current) != request_identity(&agent) {
            return Err(AgentReadError::Unavailable(
                "original request source changed".into(),
            ));
        }
        result
    }
}

fn request_identity(agent: &Agent) -> Option<(&str, &str, &str, Option<FirstMessagePurpose>)> {
    agent.conversation.first_message.as_ref().map(|record| {
        (
            record.origin_key.as_str(),
            record.text.as_str(),
            record.payload_sha256.as_str(),
            record.purpose,
        )
    })
}

fn project(
    record: Option<&crate::sandbox::FirstMessageRecord>,
) -> Result<Option<AgentOriginalRequest>, AgentReadError> {
    let Some(record) = record else {
        return Ok(None);
    };
    record
        .validate()
        .map_err(|error| AgentReadError::Corrupt(error.to_string()))?;
    Ok(
        (record.purpose == Some(FirstMessagePurpose::Task)).then(|| AgentOriginalRequest {
            text: record.text.clone(),
            turn_sequence: record.turn_sequence,
        }),
    )
}

#[cfg(test)]
#[path = "original_request/tests.rs"]
mod tests;
