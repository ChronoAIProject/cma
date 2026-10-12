use super::*;
use crate::resource_access::ResourceActor;
use crate::trigger_domain::LaunchInitiation;

pub(super) struct CreationAttribution {
    pub actor: Option<ResourceActor>,
    pub initiation: Option<LaunchInitiation>,
}
impl CreationAttribution {
    pub fn ordinary(actor: ResourceActor) -> Self {
        Self {
            actor: Some(actor),
            initiation: None,
        }
    }
}

impl AgentService {
    pub async fn create_for_launch(
        &self,
        caller: &Caller,
        authority: Capability,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        self.create_or_reuse_for_launch(caller, authority, workspace_id, request, false)
            .await
    }

    pub async fn create_or_reuse_for_launch(
        &self,
        caller: &Caller,
        authority: Capability,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
        reuse_existing: bool,
    ) -> Result<Agent, AgentServiceError> {
        self.create_or_reuse_for_attributed_launch(
            caller,
            authority,
            workspace_id,
            request,
            reuse_existing,
            None,
        )
        .await
    }

    pub async fn create_or_reuse_for_attributed_launch(
        &self,
        caller: &Caller,
        authority: Capability,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
        reuse_existing: bool,
        initiation: Option<LaunchInitiation>,
    ) -> Result<Agent, AgentServiceError> {
        require_launch(caller, authority)?;
        let AgentOrigin::Trigger { launch_id, .. } = &request.origin else {
            return Err(AgentServiceError::InvalidRequest(
                "launch admission requires trigger origin".into(),
            ));
        };
        let actor = crate::resource_access::ResourceActor::Trigger {
            subject: caller.subject.clone(),
            launch_id: crate::LaunchId(launch_id.clone()),
        };
        self.create_with_profile(
            caller,
            workspace_id,
            request,
            true,
            reuse_existing,
            CreationAttribution {
                actor: Some(actor),
                initiation,
            },
        )
        .await
    }
}
