//! Public media resolution authorizes before routing; mutations retain the
//! receiving AM's exact authority through upload and command publication.
use super::MediaContinuations;
use layer1_public::{
    rest::binding_media::{BindingMediaError as Error, BindingMediaPath},
    AuthenticatedResourceCaller,
};
use layer2_core::{
    agent_ownership::{execution_authority, Authority},
    resource_access::{ResourceCapability, ResourceTarget},
    AgentId,
};
use std::{future::Future, sync::Arc};

impl MediaContinuations {
    pub(crate) async fn owner_target(
        &self,
        caller: &AuthenticatedResourceCaller,
        path: &BindingMediaPath,
    ) -> Result<AgentId, Error> {
        let (parent, authorized) = self.parent_for_caller(caller, path).await?;
        let id = parent.receipt.envelope.agent_id;
        self.access
            .validate_read_result(
                &authorized,
                &ResourceTarget::Agent {
                    agent_id: id.clone(),
                },
                ResourceCapability::Interact,
            )
            .await
            .map_err(super::parent::resource_error)?;
        Ok(id)
    }

    pub(super) fn capture_owner(&self, id: &AgentId) -> Result<Option<Arc<Authority>>, Error> {
        if self.ownership.get().is_none() {
            return Ok(None);
        }
        let authority = execution_authority().ok_or(Error::Unavailable)?;
        self.require_owner(id, &authority)?;
        Ok(Some(authority))
    }

    pub(super) fn require_owner(&self, id: &AgentId, authority: &Authority) -> Result<(), Error> {
        let coordinator = self.ownership.get().ok_or(Error::Unavailable)?;
        if !coordinator.is_ready()
            || !authority.is_current()
            || authority.token.agent_id != *id
            || authority.token.instance_id != coordinator.instance_id
        {
            return Err(Error::Unavailable);
        }
        Ok(())
    }
}

pub(super) async fn scoped<T>(
    authority: Option<&Arc<Authority>>,
    operation: impl Future<Output = Result<T, Error>>,
) -> Result<T, Error> {
    match authority {
        Some(authority) => authority
            .run(operation)
            .await
            .map_err(|_| Error::Unavailable)?,
        None => operation.await,
    }
}
