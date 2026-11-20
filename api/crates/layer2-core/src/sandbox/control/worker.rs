use super::*;
use crate::ids::SandboxCommandId;
use crate::resource_access::{
    AccessError, MembershipError, ResourceAccessError, ResourceAccessService,
    ResourceAccessStoreError,
};
use crate::sandbox::agent_io::{CommandAuthorizationError, ResourceCommandAuthorizationError};
use crate::sandbox::store::{Page, PageRequest};
use crate::sandbox::{SandboxCredentialPrerequisite, SandboxCredentialPrerequisiteError};
use std::{sync::Arc, time::Duration};

/// Select the actual requesting actor's retained sign-in, verify its current
/// capabilities and resource policy, then recheck exact custody after all IO.
/// An owner ID or historical receipt is never an authenticated caller.
#[async_trait::async_trait]
pub trait SandboxControlAuthorizer: Send + Sync {
    async fn authorize_sandbox_control(
        &self,
        access: &ResourceAccessService,
        record: &SandboxControlRecord,
    ) -> Result<Arc<SandboxControlAuthorization>, ResourceCommandAuthorizationError>;
}

pub struct SandboxControlWorker {
    store: Arc<dyn SandboxControlStore>,
    access: Arc<ResourceAccessService>,
    authority: Arc<dyn SandboxControlAuthorizer>,
    credentials: Arc<dyn SandboxCredentialPrerequisite>,
}
impl SandboxControlWorker {
    pub fn new(
        store: Arc<dyn SandboxControlStore>,
        access: Arc<ResourceAccessService>,
        authority: Arc<dyn SandboxControlAuthorizer>,
        credentials: Arc<dyn SandboxCredentialPrerequisite>,
    ) -> Arc<Self> {
        Arc::new(Self {
            store,
            access,
            authority,
            credentials,
        })
    }
    pub async fn discover(
        &self,
        page: &PageRequest,
    ) -> Result<Page<SandboxControlRecord>, SandboxControlError> {
        tokio::time::timeout(
            Duration::from_secs(15),
            self.store.queued_sandbox_controls(page),
        )
        .await
        .map_err(|_| SandboxControlError::Unavailable)?
    }
    pub async fn run_one(&self, id: &SandboxCommandId) -> Result<(), SandboxControlError> {
        tokio::time::timeout(Duration::from_secs(45), self.run_inner(id))
            .await
            .map_err(|_| SandboxControlError::Unavailable)?
    }
    async fn run_inner(&self, id: &SandboxCommandId) -> Result<(), SandboxControlError> {
        let Some(record) = self.store.sandbox_control(id).await? else {
            return Ok(());
        };
        record.validate()?;
        if !record.queued() {
            return Ok(());
        }
        if record.expired(&crate::now_rfc3339())? {
            return self
                .reject(&record, SandboxControlFailure::QueueExpired)
                .await;
        }
        let proof = match self
            .authority
            .authorize_sandbox_control(&self.access, &record)
            .await
        {
            Ok(proof) => proof,
            Err(error) => return self.authority_failure(&record, error).await,
        };
        let proof = if record.envelope.command == SandboxControl::Revive {
            let snapshot = proof.current_snapshot().await.map_err(resource_failure)?;
            let sandbox = snapshot.lineage.sandbox;
            let connections =
                self.credentials
                    .check_services(
                        &sandbox.owner,
                        &sandbox.profile.content.connection_services_for_grants(
                            &sandbox.credential.effective_reaches(),
                        ),
                    )
                    .await;
            // Verify custody and resource authority again even when the provider
            // check failed. Do not expose connection details after access loss.
            let fresh = match self
                .authority
                .authorize_sandbox_control(&self.access, &record)
                .await
            {
                Ok(fresh) => fresh.bound_by(&proof).map_err(resource_failure)?,
                Err(error) => return self.authority_failure(&record, error).await,
            };
            match connections {
                Ok(()) => fresh,
                Err(SandboxCredentialPrerequisiteError::ConnectionsRequired { .. }) => {
                    return self
                        .reject(&record, SandboxControlFailure::ConnectionsRequired)
                        .await
                }
                Err(SandboxCredentialPrerequisiteError::SetupRequired) => {
                    return self
                        .reject(&record, SandboxControlFailure::CredentialSetupRequired)
                        .await
                }
                Err(SandboxCredentialPrerequisiteError::Unavailable(_)) => {
                    return Err(SandboxControlError::Unavailable)
                }
            }
        } else {
            proof
        };
        self.store.apply_sandbox_control(&record, proof).await?;
        Ok(())
    }
    async fn reject(
        &self,
        record: &SandboxControlRecord,
        reason: SandboxControlFailure,
    ) -> Result<(), SandboxControlError> {
        self.store.reject_sandbox_control(record, reason).await?;
        Ok(())
    }
    async fn authority_failure(
        &self,
        record: &SandboxControlRecord,
        error: ResourceCommandAuthorizationError,
    ) -> Result<(), SandboxControlError> {
        use ResourceCommandAuthorizationError::{Custody, Resource};
        if record.expired(&crate::now_rfc3339())? {
            return self
                .reject(record, SandboxControlFailure::QueueExpired)
                .await;
        }
        match error {
            Resource(ResourceAccessError::AccountDeletionInProgress)
            | Custody(CommandAuthorizationError::Revoked)
            | Resource(ResourceAccessError::Access(
                AccessError::NotFound | AccessError::Forbidden,
            ))
            | Resource(ResourceAccessError::Membership(MembershipError::NotMember)) => {
                self.reject(record, SandboxControlFailure::AuthorityRevoked)
                    .await
            }
            Resource(ResourceAccessError::Store(ResourceAccessStoreError::NotFound)) => {
                self.reject(record, SandboxControlFailure::TargetChanged)
                    .await
            }
            Resource(error) => Err(resource_failure(error)),
            Custody(CommandAuthorizationError::Unavailable) => {
                Err(SandboxControlError::Unavailable)
            }
        }
    }
}
fn resource_failure(error: ResourceAccessError) -> SandboxControlError {
    match error {
        ResourceAccessError::Store(ResourceAccessStoreError::Corrupt) => {
            SandboxControlError::Corrupt
        }
        ResourceAccessError::Store(ResourceAccessStoreError::Conflict)
        | ResourceAccessError::Access(AccessError::RevalidationRequired) => {
            SandboxControlError::Conflict
        }
        _ => SandboxControlError::Unavailable,
    }
}
