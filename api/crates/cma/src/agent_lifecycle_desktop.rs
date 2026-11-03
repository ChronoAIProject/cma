use super::*;
use layer2_core::sandbox::desktop_creation::DesktopCapability;

impl InlineAgentLifecycle {
    pub(super) async fn read_desktop_capability(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        scope: layer1_public::rest::DesktopCreationScope,
    ) -> Result<DesktopCapability, AgentLifecycleProvisioningError> {
        self.authority(caller, context)?;
        if !caller.authorization.allows(Capability::UseOwnSandboxes)
            || (scope.workspace_id.is_none()
                && scope.sandbox_id.is_none()
                && !caller.authorization.allows(Capability::CreateSandbox))
        {
            return Err(AgentLifecycleProvisioningError::Forbidden);
        }
        let request = AgentLifecycleCreateRequest {
            name: "Desktop capability".into(),
            initial_message: None,
            desktop: true,
            workspace_id: scope.workspace_id,
            sandbox_id: scope.sandbox_id,
            image: None,
            repository: None,
            profile: None,
        };
        validate_request(&request)?;
        self.inspect_desktop(caller, context, &request, None).await
    }

    pub(super) async fn inspect_desktop(
        &self,
        caller: &Caller,
        context: Option<&CallerContext>,
        request: &AgentLifecycleCreateRequest,
        image: Option<&str>,
    ) -> Result<DesktopCapability, AgentLifecycleProvisioningError> {
        let parent = if request.workspace_id.is_some() {
            Some(
                self.contextual_workspace(caller, context.map(|value| &value.actor), request)
                    .await?
                    .1,
            )
        } else if request.sandbox_id.is_some() {
            Some(
                self.contextual_sandbox(caller, context.map(|value| &value.actor), request)
                    .await?,
            )
        } else {
            None
        };
        match &self.desktop {
            Some(service) => Ok(service
                .capability(image, self.capacity, parent.as_ref())
                .await),
            None => Ok(DesktopCapability::unavailable(
                "This deployment has no verified desktop runtime.",
            )),
        }
    }

    pub(super) async fn wait_for_prepared_agent(
        &self,
        caller: &Caller,
        agent: &AgentId,
        lease: &LifecycleLease,
        desktop: bool,
    ) -> Result<(), AgentLifecycleProvisioningError> {
        if desktop {
            self.progress(
                lease,
                "desktop_preparation",
                "running",
                "Preparing the Agent desktop; the first task is retained",
            )
            .await?;
        }
        self.wait_for_agent_ready(caller, agent).await?;
        if desktop {
            self.progress(
                lease,
                "desktop_preparation",
                "ready",
                "Desktop prepared; Live requires an observed frame",
            )
            .await?;
        }
        Ok(())
    }
}
