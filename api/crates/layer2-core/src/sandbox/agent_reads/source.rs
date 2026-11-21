use crate::resource_access::{CallerContext, ResourceCapability};
use crate::trigger_domain::{AgentTriggerSource, TriggerService, TriggerServiceError};

use super::{authorized, AgentId, AgentReadError, AgentReadService, Caller, ResourceTarget};
use crate::sandbox::AgentOrigin;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum AgentSourceReading {
    Personal {
        agent_id: AgentId,
        origin: AgentOrigin,
        trigger: Option<AgentTriggerSource>,
    },
    Shared {
        agent_id: AgentId,
        origin: SharedAgentOrigin,
    },
}

/// Creation category only: private Launch/Trigger IDs and profile indexes are
/// outside a shared Agent's public source projection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedAgentOrigin {
    Manual,
    WorkspaceProfile,
    Trigger,
}

impl From<&AgentOrigin> for SharedAgentOrigin {
    fn from(origin: &AgentOrigin) -> Self {
        match origin {
            AgentOrigin::Manual | AgentOrigin::Lifecycle { .. } => Self::Manual,
            AgentOrigin::WorkspaceProfile { .. } => Self::WorkspaceProfile,
            AgentOrigin::Trigger { .. } => Self::Trigger,
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum AgentSourceReadError {
    #[error(transparent)]
    Agent(#[from] AgentReadError),
    #[error(transparent)]
    Trigger(#[from] TriggerServiceError),
}

impl AgentReadService {
    pub async fn source_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        triggers: &TriggerService,
    ) -> Result<AgentSourceReading, AgentSourceReadError> {
        let value = self
            .authorized(context, credential, id, ResourceCapability::ViewDetails)
            .await?;
        let agent = value.snapshot.lineage.agent.as_ref().ok_or_else(|| {
            AgentReadError::Corrupt("Agent source has no authoritative Agent".into())
        })?;
        let result = if authorized::is_personal(&value, context) {
            triggers
                .agent_source(&context.caller, agent)
                .await
                .map(|trigger| AgentSourceReading::Personal {
                    agent_id: agent.id.clone(),
                    origin: agent.origin.clone(),
                    trigger,
                })
        } else {
            // Do not read another owner's Launch or publication dependencies.
            Ok(AgentSourceReading::Shared {
                agent_id: agent.id.clone(),
                origin: (&agent.origin).into(),
            })
        };
        // Fence success and dependency errors after IO before releasing either.
        self.finish_read(&value, id, ResourceCapability::ViewDetails)
            .await?;
        Ok(result?)
    }

    /// Compatibility path for authenticated callers without a resource context.
    /// Explicit organization ownership never becomes personal via a raw ID.
    pub async fn source(
        &self,
        caller: &Caller,
        id: &AgentId,
        triggers: &TriggerService,
    ) -> Result<AgentSourceReading, AgentSourceReadError> {
        super::require(caller)?;
        let held = if let Some(access) = &self.access {
            Some(
                access
                    .authorize_personal(
                        caller,
                        &ResourceTarget::Agent {
                            agent_id: id.clone(),
                        },
                        ResourceCapability::ViewDetails,
                    )
                    .await
                    .map_err(authorized::access_error)?,
            )
        } else {
            None
        };
        let agent = match &held {
            Some(value) => value.snapshot.lineage.agent.clone().ok_or_else(|| {
                AgentReadError::Corrupt("Agent source has no authoritative Agent".into())
            })?,
            None => self.read(caller, id).await?,
        };
        let result = triggers.agent_source(caller, &agent).await;
        if let Some(value) = &held {
            self.finish_read(value, id, ResourceCapability::ViewDetails)
                .await?;
        }
        Ok(AgentSourceReading::Personal {
            agent_id: agent.id,
            origin: agent.origin,
            trigger: result?,
        })
    }
}
