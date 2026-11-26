//! Runtime publishing derives ownership from the confirmed native conversation.
use super::*;
use crate::{
    sandbox::agent_access::AgentAccessError,
    sandbox::agent_access_tool::{AgentAccessRequester, AgentAccessToolAuthenticator},
    Authorization, Caller, Capability,
};

pub(super) struct RuntimeGuard<'a> {
    auth: &'a dyn AgentAccessToolAuthenticator,
    capability: &'a str,
    requester: AgentAccessRequester,
}
impl RuntimeGuard<'_> {
    pub(super) async fn validate(
        &self,
        record: Option<&StoredDocument>,
    ) -> Result<(), DocumentError> {
        let current = self
            .auth
            .authenticate(self.capability)
            .await
            .map_err(authority_error)?;
        if current != self.requester
            || record.is_some_and(|r| {
                r.metadata.owner_subject != current.owner.as_str()
                    || r.runtime_author_agent_id.as_deref() != Some(current.agent_id.as_str())
            })
        {
            return Err(DocumentError::NotFound);
        }
        Ok(())
    }
    fn caller(&self) -> Caller {
        Caller::new(
            self.requester.owner.clone(),
            Authorization::new([Capability::UseOwnSandboxes]),
        )
    }
}
impl DocumentService {
    pub async fn deliverable_for_agent(
        &self,
        auth: &dyn AgentAccessToolAuthenticator,
        capability: &str,
        id: &str,
        revision: u64,
    ) -> Result<crate::sandbox::answer_deliverables::DeliverableDescription, DocumentError> {
        use crate::sandbox::answer_deliverables::{DeliverableDescription, DeliverableReference};
        let requester = auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?;
        let guard = RuntimeGuard {
            auth,
            capability,
            requester,
        };
        let record = self.authorize(&guard.caller(), id, None, false).await?;
        guard.validate(Some(&record)).await?;
        let metadata = record
            .revisions
            .iter()
            .find(|item| item.metadata.revision == revision)
            .ok_or(DocumentError::NotFound)?;
        let reference = DeliverableReference::document(
            guard.requester.agent_id.clone(),
            id.into(),
            &metadata.metadata,
        );
        let marker = reference.marker().map_err(|_| DocumentError::Invalid)?;
        let current = self.authorize(&guard.caller(), id, None, false).await?;
        guard.validate(Some(&current)).await?;
        Ok(DeliverableDescription { reference, marker })
    }

    /// Publish on behalf of the currently authenticated runtime owner. Caller
    /// supplied provenance is replaced with the exact confirmed Agent identity.
    pub async fn create_for_agent(
        &self,
        auth: &dyn AgentAccessToolAuthenticator,
        capability: &str,
        key: &str,
        mut body: CreateDocument,
    ) -> Result<DocumentView, DocumentError> {
        let requester = auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?;
        let key = runtime_key(&requester, key)?;
        body.origin_agent_id = Some(requester.agent_id.to_string());
        let guard = RuntimeGuard {
            auth,
            capability,
            requester,
        };
        self.create_guarded(&guard.caller(), &key, body, Some(&guard))
            .await
    }
    /// A runtime can revise only its own attributed documents; every replay and
    /// metadata commit revalidates the original runtime/conversation authority.
    pub async fn update_for_agent(
        &self,
        auth: &dyn AgentAccessToolAuthenticator,
        capability: &str,
        id: &str,
        key: &str,
        body: UpdateDocument,
    ) -> Result<DocumentView, DocumentError> {
        if body.title.as_ref().is_some_and(|v| !types::valid_title(v))
            || body
                .markdown
                .as_ref()
                .is_some_and(|v| !types::valid_markdown(v))
            || (body.title.is_none() && body.markdown.is_none())
        {
            return Err(DocumentError::Invalid);
        }
        let requester = auth
            .authenticate(capability)
            .await
            .map_err(authority_error)?;
        let key = runtime_key(&requester, key)?;
        let guard = RuntimeGuard {
            auth,
            capability,
            requester,
        };
        Ok(self
            .mutate_guarded(
                &guard.caller(),
                id,
                None,
                &key,
                super::mutation::Command::Update(body),
                Some(&guard),
            )
            .await?
            .document)
    }
}

fn authority_error(error: AgentAccessError) -> DocumentError {
    match error {
        AgentAccessError::AccountDeletionInProgress => DocumentError::AccountDeletionInProgress,
        AgentAccessError::Forbidden
        | AgentAccessError::NotFound
        | AgentAccessError::OwnerAuthorityRequired
        | AgentAccessError::ServiceConnectionRequired => DocumentError::NotFound,
        AgentAccessError::Unavailable => DocumentError::Unavailable,
        AgentAccessError::Invalid => DocumentError::Invalid,
        AgentAccessError::Conflict | AgentAccessError::StaleGithubApproval => {
            DocumentError::Conflict
        }
    }
}
fn runtime_key(requester: &AgentAccessRequester, key: &str) -> Result<String, DocumentError> {
    super::service::operation_key(key)?;
    types::fingerprint(&("runtime_document", requester.agent_id.as_str(), key))
}
