use super::MediaContinuations;
use layer1_public::{
    rest::binding_media::{BindingMediaError as Error, BindingMediaPath},
    AuthenticatedResourceCaller,
};
use layer2_core::{
    resource_access::{
        AccessPurpose, AuthorizedResource, CredentialSource, ResourceCapability, ResourceTarget,
    },
    sandbox::agent_io::{external_event_key, AdmittedAgentCommand, AgentCommandEnvelope},
};

impl MediaContinuations {
    pub(super) async fn parent_for_caller(
        &self,
        caller: &AuthenticatedResourceCaller,
        path: &BindingMediaPath,
    ) -> Result<(AdmittedAgentCommand, AuthorizedResource), Error> {
        if caller.context.credential_source != CredentialSource::Binding
            || path.attachment_index >= 32
        {
            return Err(Error::Forbidden);
        }
        let key = external_event_key(
            &format!("binding:{}", path.binding_id),
            &path.conversation_id,
            &path.event_id,
        );
        let parent = self
            .store
            .get_external_event_command(&key)
            .await
            .map_err(|_| Error::Unavailable)?
            .ok_or(Error::Forbidden)?;
        parent.validate().map_err(|_| Error::Unavailable)?;
        self.validate_parent(&parent.receipt.envelope)?;
        let credential = caller.credential.as_ref().ok_or(Error::Forbidden)?;
        let authorized = self
            .access
            .authorize(
                &caller.context,
                Some(credential.value()),
                &ResourceTarget::Agent {
                    agent_id: parent.receipt.envelope.agent_id.clone(),
                },
                ResourceCapability::Interact,
                AccessPurpose::Mutation {
                    started_at: std::time::SystemTime::now(),
                },
            )
            .await
            .map_err(resource_error)?;
        let proof = authorized.binding_authority().ok_or(Error::Forbidden)?;
        if proof.binding.binding_id != path.binding_id {
            return Err(Error::Forbidden);
        }
        proof
            .require_command(&parent.receipt.envelope)
            .map_err(|_| Error::Forbidden)?;
        Ok((parent, authorized))
    }
    pub(super) fn validate_parent(&self, parent: &AgentCommandEnvelope) -> Result<(), Error> {
        parent.validate_integrity().map_err(|_| Error::Forbidden)?;
        if parent.continuation.is_some()
            || parent.command.external_event().is_none()
            || parent.authorization.grant_revision.is_none_or(|v| v == 0)
            || !parent.authorization.authority_id.starts_with("binding:")
        {
            return Err(Error::Forbidden);
        }
        Ok(())
    }
}

pub(super) fn resource_error(error: layer2_core::resource_access::ResourceAccessError) -> Error {
    use layer2_core::resource_access::{
        AccessError, MembershipError, ResourceAccessError, ResourceAccessStoreError,
    };
    match error {
        ResourceAccessError::Store(
            ResourceAccessStoreError::Unavailable
            | ResourceAccessStoreError::Corrupt
            | ResourceAccessStoreError::Conflict,
        )
        | ResourceAccessError::Membership(MembershipError::Unavailable)
        | ResourceAccessError::Access(AccessError::RevalidationRequired) => Error::Unavailable,
        _ => Error::Forbidden,
    }
}
