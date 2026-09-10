//! Resolve prepared profile sessions through the durable Launch that adopted them.

use std::collections::BTreeSet;

use crate::sandbox::{Agent, AgentOrigin};
use crate::Caller;

use super::{
    LaunchPageRequest, LaunchService, LaunchServiceError, PublicTrigger, MAX_LAUNCH_PAGE_SIZE,
};

impl LaunchService {
    pub(super) async fn newest_prepared_trigger_agent(
        &self,
        caller: &Caller,
        trigger: &PublicTrigger,
        mut newest: Option<Agent>,
    ) -> Result<Option<Agent>, LaunchServiceError> {
        let mut page = LaunchPageRequest {
            trigger: Some(trigger.trigger_id.clone()),
            limit: MAX_LAUNCH_PAGE_SIZE,
            ..Default::default()
        };
        let mut agents = BTreeSet::new();
        let mut cursors = BTreeSet::new();
        loop {
            let rows = self
                .launches
                .list_launches(&caller.subject, &page)
                .await
                .map_err(super::launch_service_support::store_error)?;
            for launch in rows.items {
                if launch.owner != caller.subject || launch.trigger_id != trigger.trigger_id {
                    return Err(corrupt());
                }
                let Some(id) = launch.resolution.agent.id() else {
                    continue;
                };
                if !agents.insert(id.clone()) {
                    continue;
                }
                let Some(agent) = self
                    .contexts
                    .owned_agent(&caller.subject, id)
                    .await
                    .map_err(|error| LaunchServiceError::Unavailable(error.to_string()))?
                else {
                    continue;
                };
                if !launch.resolved_agent_matches(&agent) {
                    return Err(corrupt());
                }
                if !agent.status.is_live()
                    || !matches!(agent.origin, AgentOrigin::WorkspaceProfile { .. })
                    || agent.work.warm_conversation_id.is_none()
                {
                    continue;
                }
                agent.validate_conversation().map_err(|_| corrupt())?;
                if newest
                    .as_ref()
                    .is_none_or(|old| (&agent.created_at, &agent.id) > (&old.created_at, &old.id))
                {
                    newest = Some(agent);
                }
            }
            let Some(next) = rows.next else {
                return Ok(newest);
            };
            if !cursors.insert(next.clone()) {
                return Err(corrupt());
            }
            page.cursor = Some(next);
        }
    }
}

fn corrupt() -> LaunchServiceError {
    LaunchServiceError::Corrupt("Prepared Agent reuse crossed its durable Launch boundary".into())
}
