use std::{
    sync::Arc,
    time::{Duration, SystemTime},
};

use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStoreError, ResourceCapability,
    ResourceTarget, MAX_MEMBERSHIP_AGE,
};
use crate::{now_rfc3339, seconds_from_rfc3339, Caller};

use super::{AdmittedAgentCommand, AgentCommand, CommandExecutionAuthorization};

/// Permission to revive the actual parent of exactly one pending command.
/// Agent interaction and Sandbox management are independently authorized. The
/// custody adapter must verify the selected credential again after issuance.
pub struct CommandRevivalAuthorization {
    execution: CommandExecutionAuthorization,
    parent: AuthorizedResource,
    command: AdmittedAgentCommand,
    deadline: tokio::time::Instant,
}

impl std::fmt::Debug for CommandRevivalAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandRevivalAuthorization")
            .field("command_id", &self.command.receipt.envelope.command_id)
            .finish_non_exhaustive()
    }
}

impl ResourceAccessService {
    pub async fn authorize_command_revival(
        &self,
        context: &CallerContext,
        credential: &str,
        command: &AdmittedAgentCommand,
    ) -> Result<Arc<CommandRevivalAuthorization>, ResourceAccessError> {
        let deadline = revival_deadline(command)?;
        let execution = self
            .authorize_command_execution(context, credential, &command.receipt.envelope)
            .await?;
        let mut snapshot = execution.current_snapshot().await?;
        let parent = if let Some(binding) = execution.fence().binding_authority() {
            // Only this admitted Agent command may revive its proved parent.
            // The public binding principal still cannot target a Sandbox.
            snapshot.lineage.agent = None;
            snapshot.lineage.worktree = None;
            snapshot.lineage.workspace = None;
            AuthorizedResource {
                snapshot,
                decision: crate::resource_access::AccessDecision::binding(binding, true),
                caller: context.caller.clone(),
                binding: Some(binding.clone()),
            }
        } else {
            self.authorize(
                context,
                Some(credential),
                &ResourceTarget::Sandbox {
                    sandbox_id: snapshot.lineage.sandbox.id,
                },
                ResourceCapability::ManageSandbox,
                AccessPurpose::Mutation {
                    started_at: SystemTime::now(),
                },
            )
            .await?
        };
        finish(execution, parent, command, deadline).await
    }

    /// Personal compatibility cannot borrow another owner's lifecycle access.
    pub async fn authorize_personal_command_revival(
        &self,
        caller: &Caller,
        command: &AdmittedAgentCommand,
    ) -> Result<Arc<CommandRevivalAuthorization>, ResourceAccessError> {
        let deadline = revival_deadline(command)?;
        let execution = self
            .authorize_personal_command_execution(caller, &command.receipt.envelope)
            .await?;
        let snapshot = execution.current_snapshot().await?;
        let parent = self
            .authorize_personal(
                caller,
                &ResourceTarget::Sandbox {
                    sandbox_id: snapshot.lineage.sandbox.id,
                },
                ResourceCapability::ManageSandbox,
            )
            .await?;
        finish(execution, parent, command, deadline).await
    }
}

async fn finish(
    execution: CommandExecutionAuthorization,
    parent: AuthorizedResource,
    command: &AdmittedAgentCommand,
    deadline: tokio::time::Instant,
) -> Result<Arc<CommandRevivalAuthorization>, ResourceAccessError> {
    let deadline = parent
        .decision
        .revalidation_deadline(ResourceCapability::ManageSandbox)
        .map_or(deadline, |membership| deadline.min(membership));
    let proof = Arc::new(CommandRevivalAuthorization {
        execution,
        parent,
        command: command.clone(),
        deadline,
    });
    proof.current_snapshot().await?;
    Ok(proof)
}

impl CommandRevivalAuthorization {
    pub fn binding_authority(
        &self,
    ) -> Option<&crate::sandbox::binding::BindingConversationAuthority> {
        self.parent.binding_authority()
    }

    pub fn authorization_audits(
        &self,
    ) -> Result<Vec<crate::resource_access::ResourceAuthorizationAudit>, ResourceAccessError> {
        self.require_current()?;
        let target = ResourceTarget::Sandbox {
            sandbox_id: self.parent.snapshot.lineage.sandbox.id.clone(),
        };
        Ok(vec![
            self.execution.fence().authorization_audit()?,
            self.parent.authorization_audit(
                &self.command.receipt.envelope.actor,
                &target,
                ResourceCapability::ManageSandbox,
            )?,
        ])
    }

    pub fn command(&self) -> &AdmittedAgentCommand {
        &self.command
    }

    pub fn target(&self) -> ResourceTarget {
        self.execution.fence().target()
    }

    pub fn require_current(&self) -> Result<(), ResourceAccessError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AccessError::RevalidationRequired.into());
        }
        self.execution.fence().require_current()?;
        self.parent
            .decision
            .require(ResourceCapability::ManageSandbox)?;
        require_pending(&self.command)
    }

    pub fn require_command(
        &self,
        current: &AdmittedAgentCommand,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        if current != &self.command {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        require_pending(current)
    }

    pub async fn current_snapshot(&self) -> Result<ResourceAccessSnapshot, ResourceAccessError> {
        self.require_current()?;
        let snapshot = tokio::time::timeout_at(self.deadline, self.execution.current_snapshot())
            .await
            .map_err(|_| AccessError::RevalidationRequired)??;
        self.validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    /// Called with the store's current transaction snapshot before any write.
    pub fn validate_snapshot(
        &self,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        self.execution.fence().validate_snapshot(snapshot)?;
        let parent = &self.parent.snapshot;
        let parent_target = ResourceTarget::Sandbox {
            sandbox_id: snapshot.lineage.sandbox.id.clone(),
        };
        if parent.fence() != snapshot.fence()
            || parent.scope(&parent_target)?.owner != snapshot.scope(&self.target())?.owner
            || snapshot.lineage.sandbox.revision < parent.lineage.sandbox.revision
            || snapshot.lineage.sandbox.profile != parent.lineage.sandbox.profile
            || snapshot.lineage.sandbox.credential != parent.lineage.sandbox.credential
            || snapshot.lineage.sandbox.origin != parent.lineage.sandbox.origin
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        let agent = snapshot
            .lineage
            .agent
            .as_ref()
            .ok_or(ResourceAccessStoreError::Corrupt)?;
        if !agent.status.is_live()
            || agent.requested_stop_reason.is_some()
            || snapshot
                .lineage
                .sandbox
                .lifecycle
                .checkpoint_capture
                .is_some()
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }
}

fn require_pending(command: &AdmittedAgentCommand) -> Result<(), ResourceAccessError> {
    let now = now_rfc3339();
    if command.validate().is_err()
        || !matches!(
            command.receipt.envelope.command,
            AgentCommand::Message { .. }
        )
        || !command.awaiting_claim(&now)
        || command.queue_expired(&now)
    {
        return Err(ResourceAccessStoreError::Conflict.into());
    }
    Ok(())
}

fn revival_deadline(
    command: &AdmittedAgentCommand,
) -> Result<tokio::time::Instant, ResourceAccessError> {
    require_pending(command)?;
    let expires =
        seconds_from_rfc3339(&command.queue_deadline).ok_or(ResourceAccessStoreError::Corrupt)?;
    let remaining = (SystemTime::UNIX_EPOCH + Duration::from_secs(expires))
        .duration_since(SystemTime::now())
        .map_err(|_| AccessError::RevalidationRequired)?;
    Ok(tokio::time::Instant::now() + remaining.min(MAX_MEMBERSHIP_AGE))
}
