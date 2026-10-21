use crate::resource_access::{AuthorizedResource, CallerContext, ResourceCapability};
use crate::AgentId;

use super::{AgentLineage, AgentReadError, AgentReadService};

/// Internal lineage and current policy proof. REST exposes only the settings or
/// model projection, never a shared Agent's parent ownership or credentials.
pub struct AgentConfigurationRead {
    pub lineage: AgentLineage,
    authority: AuthorizedResource,
}

impl AgentReadService {
    pub async fn configuration_authorized(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
    ) -> Result<AgentConfigurationRead, AgentReadError> {
        let authority = self
            .authorized(context, credential, id, ResourceCapability::OperateAgent)
            .await?;
        let lineage = super::authorized::lineage(authority.snapshot.lineage.clone())?;
        Ok(AgentConfigurationRead { lineage, authority })
    }

    pub async fn validate_configuration_read(
        &self,
        read: &AgentConfigurationRead,
    ) -> Result<(), AgentReadError> {
        self.finish_read(
            &read.authority,
            &read.lineage.agent.id,
            ResourceCapability::OperateAgent,
        )
        .await
    }

    pub async fn authorize_settings_update(
        &self,
        context: &CallerContext,
        credential: Option<&str>,
        id: &AgentId,
        settings: &crate::sandbox::AgentSettings,
    ) -> Result<crate::sandbox::AgentSettingsAuthorization, AgentReadError> {
        self.access
            .as_ref()
            .ok_or_else(|| {
                AgentReadError::Unavailable("resource access service is not composed".into())
            })?
            .authorize_agent_settings(context, credential, id, settings)
            .await
            .map_err(super::authorized::access_error)
    }
}
