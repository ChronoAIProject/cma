use std::{sync::Arc, time::SystemTime};

use crate::resource_access::{
    AccessError, AccessPurpose, AuthorizedResource, CallerContext, ResourceAccessError,
    ResourceAccessService, ResourceAccessSnapshot, ResourceAccessStoreError,
    ResourceAuthorizationAudit, ResourceOwner, MAX_MEMBERSHIP_AGE,
};
use crate::Capability;

use super::{SandboxControl, SandboxControlEnvelope, SandboxControlRecord};

/// Opaque, nonrenewing authority for one exact lifecycle command. Stores inspect
/// their own transaction snapshot before accepting this proof. It contains no
/// credential, and serialized receipts cannot construct it.
pub struct SandboxControlAuthorization {
    access: ResourceAccessService,
    authorized: AuthorizedResource,
    envelope: SandboxControlEnvelope,
    audit: ResourceAuthorizationAudit,
    deadline: tokio::time::Instant,
}

impl std::fmt::Debug for SandboxControlAuthorization {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("SandboxControlAuthorization")
            .field("command_id", &self.envelope.command_id)
            .finish_non_exhaustive()
    }
}

impl ResourceAccessService {
    pub async fn authorize_sandbox_control(
        &self,
        context: &CallerContext,
        credential: &str,
        record: &SandboxControlRecord,
    ) -> Result<Arc<SandboxControlAuthorization>, ResourceAccessError> {
        record
            .validate()
            .map_err(|_| ResourceAccessStoreError::Corrupt)?;
        let envelope = &record.envelope;
        if !record.queued() {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        require_actor(context, envelope)?;
        let started = SystemTime::now();
        let expires = SystemTime::UNIX_EPOCH
            + std::time::Duration::from_secs(
                crate::seconds_from_rfc3339(&record.queue_deadline)
                    .ok_or(ResourceAccessStoreError::Corrupt)?,
            );
        let remaining = expires
            .duration_since(started)
            .map_err(|_| AccessError::RevalidationRequired)?;
        let deadline = tokio::time::Instant::now() + remaining.min(MAX_MEMBERSHIP_AGE);
        let authorized = self
            .authorize(
                context,
                Some(credential),
                &envelope.target(),
                envelope.command.capability(),
                AccessPurpose::Mutation {
                    started_at: started,
                },
            )
            .await?;
        SandboxControlAuthorization::bind(
            self.clone(),
            authorized,
            envelope.clone(),
            context,
            deadline,
        )
    }
}

impl SandboxControlAuthorization {
    /// A second check after dependency IO cannot extend the first observation's
    /// lifetime or change the owner/profile whose connections were checked.
    pub fn bound_by(&self, prior: &Self) -> Result<Arc<Self>, ResourceAccessError> {
        if self.envelope != prior.envelope {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        self.require_current()?;
        prior.validate_snapshot(&self.authorized.snapshot)?;
        Ok(Arc::new(Self {
            access: self.access.clone(),
            authorized: AuthorizedResource {
                binding: self.authorized.binding.clone(),
                caller: self.authorized.caller.clone(),
                snapshot: self.authorized.snapshot.clone(),
                decision: self.authorized.decision.clone(),
            },
            envelope: self.envelope.clone(),
            audit: self.audit.clone(),
            deadline: self.deadline.min(prior.deadline),
        }))
    }

    pub(super) fn bind(
        access: ResourceAccessService,
        authorized: AuthorizedResource,
        envelope: SandboxControlEnvelope,
        context: &CallerContext,
        deadline: tokio::time::Instant,
    ) -> Result<Arc<Self>, ResourceAccessError> {
        require_actor(context, &envelope)?;
        let deadline = authorized
            .decision
            .revalidation_deadline(envelope.command.capability())
            .map_or(deadline, |at| at.min(deadline));
        let deadline = context
            .credential_deadline()
            .map_or(deadline, |expiry| deadline.min(expiry));
        let audit = authorized.authorization_audit(
            &envelope.actor,
            &envelope.target(),
            envelope.command.capability(),
        )?;
        let proof = Arc::new(Self {
            access,
            authorized,
            envelope,
            audit,
            deadline,
        });
        proof.validate_snapshot(&proof.authorized.snapshot)?;
        Ok(proof)
    }

    pub fn envelope(&self) -> &SandboxControlEnvelope {
        &self.envelope
    }

    pub fn require_current(&self) -> Result<(), ResourceAccessError> {
        if tokio::time::Instant::now() >= self.deadline {
            return Err(AccessError::RevalidationRequired.into());
        }
        self.authorized
            .decision
            .require(self.envelope.command.capability())?;
        Ok(())
    }

    pub async fn current_snapshot(&self) -> Result<ResourceAccessSnapshot, ResourceAccessError> {
        self.require_current()?;
        let snapshot = tokio::time::timeout_at(
            self.deadline,
            self.access.current_read_snapshot(
                &self.authorized,
                &self.envelope.target(),
                self.envelope.command.capability(),
            ),
        )
        .await
        .map_err(|_| AccessError::RevalidationRequired)??;
        self.validate_snapshot(&snapshot)?;
        Ok(snapshot)
    }

    pub fn validate_snapshot(
        &self,
        snapshot: &ResourceAccessSnapshot,
    ) -> Result<(), ResourceAccessError> {
        self.require_current()?;
        let sandbox = &snapshot.lineage.sandbox;
        let original = &self.authorized.snapshot;
        if snapshot.scope(&self.envelope.target())?.owner != self.envelope.owner
            || sandbox.id != self.envelope.sandbox_id
            || sandbox.created_at != self.envelope.sandbox_created_at
            || snapshot.fence() != original.fence()
            || sandbox.profile != original.lineage.sandbox.profile
            || sandbox.credential != original.lineage.sandbox.credential
            || sandbox.origin != original.lineage.sandbox.origin
        {
            return Err(ResourceAccessStoreError::Conflict.into());
        }
        Ok(())
    }

    pub fn audit(&self) -> Result<ResourceAuthorizationAudit, ResourceAccessError> {
        self.require_current()?;
        Ok(self.audit.clone())
    }
}

fn require_actor(
    context: &CallerContext,
    envelope: &SandboxControlEnvelope,
) -> Result<(), ResourceAccessError> {
    if !context.validate_principal() || context.actor != envelope.actor {
        return Err(AccessError::NotFound.into());
    }
    // Preserve the existing personal delete capability. An owning organization
    // admin receives grant-management authority from current organization policy;
    // a recipient never receives it merely through a whole-Sandbox share.
    if envelope.command == SandboxControl::Retire
        && matches!(&envelope.owner, ResourceOwner::Person { subject } if subject == &context.caller.subject)
        && !context
            .caller
            .authorization
            .allows(Capability::DeleteOwnSandbox)
    {
        return Err(AccessError::Forbidden.into());
    }
    Ok(())
}
