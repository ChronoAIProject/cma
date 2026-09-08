use crate::sandbox::{Agent, AgentOrigin};
use crate::{Caller, Capability, LaunchId};

use super::{TriggerAvailability, TriggerQuickMessage, TriggerService, TriggerServiceError};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentTriggerSource {
    pub public_id: String,
    pub title: String,
    pub revision: u32,
    pub quick_messages: Vec<TriggerQuickMessage>,
}

impl TriggerService {
    /// Original provenance remains fixed; explicit message shortcuts follow publication.
    pub async fn agent_source(
        &self,
        caller: &Caller,
        agent: &Agent,
    ) -> Result<Option<AgentTriggerSource>, TriggerServiceError> {
        if !caller.authorization.allows(Capability::UseOwnSandboxes) {
            return Err(TriggerServiceError::Forbidden);
        }
        if agent.owner != caller.subject {
            return Err(TriggerServiceError::NotFound);
        }
        let AgentOrigin::Trigger {
            trigger_id,
            launch_id,
        } = &agent.origin
        else {
            return Ok(None);
        };
        let Some(launch) = self
            .launches
            .launch(&LaunchId(launch_id.clone()), &caller.subject)
            .await
            .map_err(super::trigger_service::launch_store_error)?
        else {
            return Ok(None);
        };
        if launch.id.as_str() != launch_id
            || launch.owner != caller.subject
            || launch.trigger_id.as_str() != trigger_id
            || launch
                .resolution
                .agent
                .id()
                .is_some_and(|id| id != &agent.id)
        {
            return Err(TriggerServiceError::Corrupt(
                "Agent source does not match its Launch".into(),
            ));
        }
        let Some(public) = self
            .definitions
            .trigger_by_public_id(&launch.public_id)
            .await
            .map_err(TriggerServiceError::from)?
        else {
            return Ok(None);
        };
        if public.trigger_id != launch.trigger_id || public.public_id != launch.public_id {
            return Err(TriggerServiceError::Corrupt(
                "Agent source crossed its Trigger boundary".into(),
            ));
        }
        public
            .revision
            .validate()
            .map_err(|error| TriggerServiceError::Corrupt(error.to_string()))?;
        Ok(Some(AgentTriggerSource {
            public_id: public.public_id.to_string(),
            title: public.revision.content.presentation.title,
            revision: public.revision.revision,
            quick_messages: if public.availability == TriggerAvailability::Published {
                public.revision.content.quick_messages
            } else {
                Vec::new()
            },
        }))
    }
}
