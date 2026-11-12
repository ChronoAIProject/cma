use std::{sync::Arc, time::SystemTime};

use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStoreError, ResourceCapability,
    ResourceTarget, MAX_MEMBERSHIP_AGE,
};
use crate::{Caller, Capability};

use super::{AgentActor, AgentCommandEnvelope, AgentOwner, AgentSourceKind};

/// Opaque, nonrenewing permission for exactly one command. No bearer or renewable
/// credential is retained. The custody adapter must independently authenticate
/// the actual actor before asking the resource service to issue this proof.
#[derive(Clone)]
pub struct CommandExecutionAuthorization {
    caller: Caller,
    access: ResourceAccessService,
    fence: Arc<CommandExecutionFence>,
    source_guard: Option<Arc<dyn super::CommandSourceGuard>>,
}

/// Consumed inside the same store transaction as the dispatch claim and parent
/// write. Its private fields prevent callers from supplying an unchecked policy,
/// changing the command identity or extending the captured authority lifetime.
pub struct CommandExecutionFence {
    authorized: AuthorizedResource,
    envelope: AgentCommandEnvelope,
    capability: ResourceCapability,
    deadline: tokio::time::Instant,
}

impl std::fmt::Debug for CommandExecutionFence {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("CommandExecutionFence")
            .field("command_id", &self.envelope.command_id)
            .field("agent_id", &self.envelope.agent_id)
            .field("capability", &self.capability)
            .finish_non_exhaustive()
    }
}

impl ResourceAccessService {
    /// Obtain fresh mutation evidence using the authenticated actor's credential.
    /// Resource ownership remains independent of that actor's credential custody.
    pub async fn authorize_command_execution(
        &self,
        context: &CallerContext,
        credential: &str,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceAccessError> {
        validate_actor(&context.caller, envelope)?;
        if !context.validate_principal() || context.actor != envelope.actor {
            return Err(AccessError::NotFound.into());
        }
        let started_at = SystemTime::now();
        let deadline = tokio::time::Instant::now() + MAX_MEMBERSHIP_AGE;
        let deadline = context
            .credential_deadline()
            .map_or(deadline, |expiry| deadline.min(expiry));
        let authorized = self
            .authorize(
                context,
                Some(credential),
                &target(envelope),
                super::admission_service::command_capability(&envelope.command),
                AccessPurpose::Mutation { started_at },
            )
            .await?;
        self.execution_authorization(context.caller.clone(), envelope, authorized, deadline)
    }

    /// Compatibility for authorizers that only supply personal custody. This
    /// cannot authorize a shared resource, an org owner or a substituted actor.
    pub async fn authorize_personal_command_execution(
        &self,
        caller: &Caller,
        envelope: &AgentCommandEnvelope,
    ) -> Result<CommandExecutionAuthorization, ResourceAccessError> {
        validate_actor(caller, envelope)?;
        if envelope.owner
            != (AgentOwner::Person {
                subject: caller.subject.clone(),
            })
            || matches!(
                &envelope.actor,
                AgentActor::Api {
                    delegation_id: Some(_),
                    ..
                }
            )
        {
            return Err(AccessError::NotFound.into());
        }
        let deadline = tokio::time::Instant::now() + MAX_MEMBERSHIP_AGE;
        let authorized = self
            .authorize_personal(
                caller,
                &target(envelope),
                super::admission_service::command_capability(&envelope.command),
            )
            .await?;
        self.execution_authorization(caller.clone(), envelope, authorized, deadline)
    }

    fn execution_authorization(
        &self,
        caller: Caller,
        envelope: &AgentCommandEnvelope,
        authorized: AuthorizedResource,
        deadline: tokio::time::Instant,
    ) -> Result<CommandExecutionAuthorization, ResourceAccessError> {
        if authorized.snapshot.scope(&target(envelope))?.owner != envelope.owner {
            return Err(AccessError::NotFound.into());
        }
        match authorized.binding_authority() {
            Some(proof) => proof
                .require_command(envelope)
                .map_err(|_| ResourceAccessStoreError::Conflict)?,
            None if envelope.authorization.authority_id.starts_with("binding:") => {
                return Err(AccessError::NotFound.into())
            }
            None => {}
        }
        let capability = super::admission_service::command_capability(&envelope.command);
        let deadline = authorized
            .decision
            .revalidation_deadline(capability)
            .map_or(deadline, |membership| membership.min(deadline));
        let fence = Arc::new(CommandExecutionFence {
            authorized,
            envelope: envelope.clone(),
            capability,
            deadline,
        });
        fence.require_current()?;
        Ok(CommandExecutionAuthorization {
            caller,
            access: self.clone(),
            fence,
            source_guard: None,
        })
    }
}

impl CommandExecutionAuthorization {
    pub(in crate::sandbox) fn continuation_envelope(&self) -> Option<&AgentCommandEnvelope> {
        self.fence
            .envelope
            .continuation
            .as_ref()
            .map(|_| &self.fence.envelope)
    }
    pub fn with_source_guard(mut self, guard: Arc<dyn super::CommandSourceGuard>) -> Self {
        self.source_guard = Some(guard);
        self
    }

    pub fn require_source_guard(&self) -> Result<(), ResourceAccessError> {
        if self.fence.envelope.continuation.is_some() && self.source_guard.is_none() {
            return Err(AccessError::NotFound.into());
        }
        Ok(())
    }
    pub fn caller(&self) -> &Caller {
        &self.caller
    }

    pub fn fence(&self) -> Arc<CommandExecutionFence> {
        self.fence.clone()
    }

    pub async fn current_snapshot(&self) -> Result<ResourceAccessSnapshot, ResourceAccessError> {
        self.fence.require_current()?;
        self.require_source_guard()?;
        if let Some(guard) = &self.source_guard {
            tokio::time::timeout_at(self.fence.deadline, guard.check(&self.fence.envelope))
                .await
                .map_err(|_| ResourceAccessStoreError::Unavailable)?
                .map_err(|error| match error {
                    super::CommandAuthorizationError::Revoked => {
                        ResourceAccessError::from(AccessError::NotFound)
                    }
                    super::CommandAuthorizationError::Unavailable => {
                        ResourceAccessError::from(ResourceAccessStoreError::Unavailable)
                    }
                })?;
        }
        let snapshot = tokio::time::timeout_at(
            self.fence.deadline,
            self.access.current_read_snapshot(
                &self.fence.authorized,
                &self.fence.target(),
                self.fence.capability,
            ),
        )
        .await
        .map_err(|_| AccessError::RevalidationRequired)??;
        self.fence.validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }
}

impl CommandExecutionFence {
    pub fn binding_authority(
        &self,
    ) -> Option<&crate::sandbox::binding::BindingConversationAuthority> {
        self.authorized.binding_authority()
    }

    pub fn authorization_audit(
        &self,
    ) -> Result<crate::resource_access::ResourceAuthorizationAudit, ResourceAccessError> {
        self.require_current()?;
        self.authorized
            .authorization_audit(&self.envelope.actor, &self.target(), self.capability)
    }

    pub fn target(&self) -> ResourceTarget {
        target(&self.envelope)
    }

    pub fn require_current(&self) -> Result<(), ResourceAccessError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AccessError::RevalidationRequired.into());
        }
        self.authorized.decision.require(self.capability)?;
        Ok(())
    }

    pub fn require_envelope(
        &self,
        envelope: &AgentCommandEnvelope,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        if self.envelope != *envelope {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }

    /// The store supplies this aggregate from its own current transaction. This
    /// also refuses ID reuse, moved ancestry and changed pinned Agent profiles.
    pub fn validate_snapshot(
        &self,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        let original = &self.authorized.snapshot;
        if snapshot.fence() != original.fence()
            || snapshot.scope(&self.target())? != original.scope(&self.target())?
            || !same_ancestry(&original.lineage, &snapshot.lineage)
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }
}

fn validate_actor(
    caller: &Caller,
    envelope: &AgentCommandEnvelope,
) -> Result<(), ResourceAccessError> {
    if envelope.validate_integrity().is_err()
        || !matches!(
            envelope.source.kind,
            AgentSourceKind::Api | AgentSourceKind::UserInterface
        )
        || !(caller.authorization.allows(Capability::UseOwnSandboxes)
            || caller.authorization
                == crate::Authorization::new([Capability::UseBindingConversations])
                && envelope.authorization.authority_id.starts_with("binding:")
                && envelope.source.kind == AgentSourceKind::Api)
        || envelope.actor.ordinary_subject() != Some(&caller.subject)
        || (matches!(
            &envelope.actor,
            AgentActor::Api {
                delegation_id: Some(_),
                ..
            }
        ) && (envelope.source.kind != AgentSourceKind::Api
            || !envelope
                .authorization
                .authority_id
                .starts_with("delegated:")))
    {
        return Err(AccessError::NotFound.into());
    }
    Ok(())
}

fn target(envelope: &AgentCommandEnvelope) -> ResourceTarget {
    ResourceTarget::Agent {
        agent_id: envelope.agent_id.clone(),
    }
}

pub(in crate::sandbox) fn operation_error(
    error: ResourceAccessError,
) -> crate::sandbox::AgentOperationError {
    use crate::sandbox::AgentOperationError;
    match error {
        ResourceAccessError::AccountDeletionInProgress | ResourceAccessError::Access(_) => {
            AgentOperationError::Forbidden
        }
        ResourceAccessError::Store(ResourceAccessStoreError::NotFound) => {
            AgentOperationError::NotFound
        }
        ResourceAccessError::Store(ResourceAccessStoreError::Conflict) => {
            AgentOperationError::Conflict
        }
        ResourceAccessError::Store(ResourceAccessStoreError::Corrupt) => {
            AgentOperationError::Corrupt("resource authorization data is inconsistent".into())
        }
        _ => {
            AgentOperationError::Unavailable("resource authorization could not be verified".into())
        }
    }
}

pub(in crate::sandbox) fn same_ancestry(
    original: &crate::resource_access::ResourceLineage,
    current: &crate::resource_access::ResourceLineage,
) -> bool {
    let (Some(a), Some(b), Some(w), Some(x), Some(t), Some(u)) = (
        &original.agent,
        &current.agent,
        &original.workspace,
        &current.workspace,
        &original.worktree,
        &current.worktree,
    ) else {
        return false;
    };
    a.id == b.id
        && a.created_at == b.created_at
        && a.owner == b.owner
        && a.sandbox_id == b.sandbox_id
        && a.workspace_id == b.workspace_id
        && a.worktree_id == b.worktree_id
        && a.profile == b.profile
        && b.revision >= a.revision
        && w.id == x.id
        && w.created_at == x.created_at
        && x.revision >= w.revision
        && t.id == u.id
        && t.path == u.path
        && t.branch == u.branch
        && t.base_commit == u.base_commit
        && t.agent_id == u.agent_id
        && u.revision >= t.revision
}
