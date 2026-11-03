//! Ordinary Agent creation preserves explicitly authenticated delegation.
use super::*;
use crate::resource_access::{CallerContext, ResourceActor};

impl AgentService {
    /// Trusted durable lifecycle continuation. The operation captured this actor
    /// only after validating its original CallerContext. Actor attribution does
    /// not grant authority; the captured caller capabilities are checked here.
    pub async fn create_for_lifecycle(
        &self,
        caller: &Caller,
        actor: Option<&ResourceActor>,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        self.create_for_lifecycle_with_purpose(
            caller,
            actor,
            workspace_id,
            request,
            super::super::FirstMessagePurpose::Setup,
        )
        .await
    }

    pub async fn create_for_lifecycle_with_purpose(
        &self,
        caller: &Caller,
        actor: Option<&ResourceActor>,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
        purpose: super::super::FirstMessagePurpose,
    ) -> Result<Agent, AgentServiceError> {
        require(caller)?;
        if actor.is_some_and(|actor| actor.ordinary_subject() != Some(&caller.subject)) {
            return Err(AgentServiceError::Forbidden);
        }
        if !matches!(request.origin, AgentOrigin::Lifecycle { .. }) {
            return Err(AgentServiceError::InvalidRequest(
                "lifecycle admission requires operation origin".into(),
            ));
        }
        self.create_with_profile(
            caller,
            workspace_id,
            request,
            true,
            false,
            CreationAttribution {
                actor: actor.cloned(),
                initiation: None,
                first_message_purpose: purpose,
            },
        )
        .await
    }

    /// Check the same owned Workspace/Sandbox admission authority before an
    /// inline lifecycle writes private profiles or persists its operation.
    pub async fn authorize_lifecycle_workspace(
        &self,
        caller: &Caller,
        actor: Option<&ResourceActor>,
        workspace_id: &WorkspaceId,
    ) -> Result<(Workspace, Sandbox), AgentServiceError> {
        require(caller)?;
        if actor.is_some_and(|actor| actor.ordinary_subject() != Some(&caller.subject)) {
            return Err(AgentServiceError::Forbidden);
        }
        self.runtime_capabilities
            .require([RuntimeCapability::Conversation])
            .map_err(|capability| AgentServiceError::CapabilityUnsupported { capability })?;
        let workspace = self.owned_workspace(caller, workspace_id).await?;
        if workspace.status != WorkspaceStatus::Ready || workspace.resolved_commit.is_none() {
            return Err(AgentServiceError::WorkspaceNotReady);
        }
        let sandbox = self.parent_sandbox(caller, &workspace).await?;
        if !sandbox.accepts_creation_admission() {
            return Err(AgentServiceError::SandboxNotReady);
        }
        self.connections.check_sandbox(&sandbox).await?;
        Ok((workspace, sandbox))
    }

    pub async fn create(
        &self,
        caller: &Caller,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        self.create_as(
            caller,
            &ResourceActor::Human {
                subject: caller.subject.clone(),
            },
            workspace_id,
            request,
        )
        .await
    }

    /// Actor attribution alone cannot establish delegated creation authority.
    pub async fn create_as(
        &self,
        caller: &Caller,
        actor: &ResourceActor,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        if !matches!(actor,
            ResourceActor::Human { subject }
            | ResourceActor::Api { subject, delegation_id: None }
            if subject == &caller.subject)
        {
            return Err(AgentServiceError::Forbidden);
        }
        self.create_manual(caller, actor, workspace_id, request)
            .await
    }

    /// Authentication supplies the exact verified grant source with its actor.
    /// Durable creation provenance retains that actor, never an owner projection.
    pub async fn create_authorized(
        &self,
        context: &CallerContext,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        if !context.validate_principal()
            || context.actor.ordinary_subject() != Some(&context.caller.subject)
        {
            return Err(AgentServiceError::Forbidden);
        }
        self.create_manual(&context.caller, &context.actor, workspace_id, request)
            .await
    }

    async fn create_manual(
        &self,
        caller: &Caller,
        actor: &ResourceActor,
        workspace_id: &WorkspaceId,
        request: CreateAgent,
    ) -> Result<Agent, AgentServiceError> {
        require(caller)?;
        if !matches!(&request.origin, AgentOrigin::Manual) {
            return Err(AgentServiceError::InvalidRequest(
                "trigger origin requires launch admission".into(),
            ));
        }
        self.create_with_profile(
            caller,
            workspace_id,
            request,
            false,
            false,
            CreationAttribution::ordinary(actor.clone()),
        )
        .await
    }
}
