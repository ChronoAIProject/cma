//! Limited consent-request capabilities, separate from all NyxID credentials.
use std::sync::Arc;

use layer2_core::sandbox::{AgentCreationProvenance, RuntimeBinding};
use layer2_core::{AgentId, SandboxId, SubjectId};
use layer3_adapter::hosted::{HostedAccessToolBinding, HostedAccessToolSource};
use layer3_adapter::{AdapterError, AgentOpenRequest, RuntimeHandle};
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use super::sandbox_mongo::{
    commit_transaction, is_duplicate_key, is_transient, read_record, record_bson,
    start_transaction, CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION,
    TRANSACTION_ATTEMPTS,
};
use super::AuthEncryptionKey;

#[cfg(test)]
mod fixtures;
mod operations;
mod records;
#[cfg(test)]
mod tests;
mod validation;
use records::{OpenRecord, Scope};

pub(super) const COLLECTION: &str = "agent_access_tools";

/// This identity can request consent only. Owner approval remains a separate
/// authenticated command; never treat this as authority to change any grant.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub(crate) struct AuthenticatedAgentAccess {
    pub agent_id: AgentId,
    pub owner: SubjectId,
    pub sandbox_id: SandboxId,
    pub conversation: String,
    pub runtime: RuntimeBinding,
    pub creation: AgentCreationProvenance,
}

pub(crate) struct MongoAgentAccessToolSource {
    store: Arc<MongoSandboxStore>,
    origin: String,
    signing_key: ring::hmac::Key,
}

impl MongoAgentAccessToolSource {
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        public_origin: &str,
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Arc<Self>, String> {
        let origin = url::Url::parse(public_origin)
            .map_err(|_| "Agent access tools need a valid HTTPS public origin")?;
        if origin.scheme() != "https"
            || origin.host_str().is_none()
            || !origin.username().is_empty()
            || origin.password().is_some()
            || origin.query().is_some()
            || origin.fragment().is_some()
            || origin.path() != "/"
        {
            return Err("Agent access tools need a clean HTTPS public origin".into());
        }
        let origin = origin.to_string();
        let namespace =
            serde_json::to_string(&[store.agent_access_tools.namespace().db, origin.clone()])
                .map_err(|_| "Agent access tools namespace could not be encoded")?;
        Ok(Arc::new(Self {
            store,
            origin,
            signing_key: encryption.agent_access_signing_key(&namespace),
        }))
    }

    pub(crate) async fn authenticate(
        &self,
        capability: &str,
    ) -> layer3_adapter::Result<AuthenticatedAgentAccess> {
        if !records::valid_capability(capability) {
            return Err(refused());
        }
        match self.run(Operation::Authenticate(capability)).await? {
            Outcome::Authenticated(identity) => Ok(*identity),
            _ => unreachable!("authentication returns an identity"),
        }
    }

    /// Bootstrap/discovery only: no conversation identity or mutation authority
    /// is returned before ACP has finished opening and publishing the session.
    pub(crate) async fn authenticate_setup(&self, capability: &str) -> layer3_adapter::Result<()> {
        if !records::valid_capability(capability) {
            return Err(refused());
        }
        self.run(Operation::Setup(capability)).await?;
        Ok(())
    }

    async fn run(&self, operation: Operation<'_>) -> layer3_adapter::Result<Outcome> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .store
                .client
                .start_session()
                .await
                .map_err(unavailable)?;
            start_transaction(&mut session).await.map_err(unavailable)?;
            let result = match operation {
                Operation::Resolve(runtime, request, fence) => self
                    .resolve_in(runtime, request, fence, &mut session)
                    .await
                    .map(Outcome::Resolved),
                Operation::Confirm(binding, conversation) => self
                    .confirm_in(binding, conversation, &mut session)
                    .await
                    .map(|()| Outcome::Confirmed),
                Operation::Authenticate(capability) => self
                    .authenticate_in(capability, &mut session)
                    .await
                    .map(|identity| Outcome::Authenticated(Box::new(identity))),
                Operation::Setup(capability) => self
                    .setup_in(capability, &mut session)
                    .await
                    .map(|()| Outcome::Confirmed),
            };
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_transaction().await;
                    match error {
                        Error::Driver(error)
                            if is_transient(&error) || is_duplicate_key(&error) =>
                        {
                            continue
                        }
                        Error::Driver(_) => return Err(unavailable(())),
                        Error::Refused => return Err(refused()),
                        Error::Unavailable => return Err(unavailable(())),
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => continue,
                _ => return Err(unavailable(())),
            }
        }
        Err(unavailable(()))
    }
}

#[async_trait::async_trait]
impl HostedAccessToolSource for MongoAgentAccessToolSource {
    async fn resolve(
        &self,
        runtime: &RuntimeHandle,
        request: &AgentOpenRequest,
        authority_fence: u64,
    ) -> layer3_adapter::Result<Option<HostedAccessToolBinding>> {
        match self
            .run(Operation::Resolve(runtime, request, authority_fence))
            .await?
        {
            Outcome::Resolved(binding) => Ok(binding),
            _ => unreachable!("resolve returns a binding"),
        }
    }

    async fn confirm_open(
        &self,
        binding: &HostedAccessToolBinding,
        conversation: &str,
    ) -> layer3_adapter::Result<()> {
        if !validation::valid_conversation(conversation) {
            return Err(refused());
        }
        self.run(Operation::Confirm(binding, conversation)).await?;
        Ok(())
    }
}

enum Operation<'a> {
    Resolve(&'a RuntimeHandle, &'a AgentOpenRequest, u64),
    Confirm(&'a HostedAccessToolBinding, &'a str),
    Authenticate(&'a str),
    Setup(&'a str),
}
enum Outcome {
    Resolved(Option<HostedAccessToolBinding>),
    Confirmed,
    Authenticated(Box<AuthenticatedAgentAccess>),
}
enum Error {
    Refused,
    Unavailable,
    Driver(mongodb::error::Error),
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
impl From<super::resource_access::ReadError> for Error {
    fn from(value: super::resource_access::ReadError) -> Self {
        match value {
            super::resource_access::ReadError::Driver(error) => Self::Driver(error),
            super::resource_access::ReadError::Domain(_) => Self::Refused,
        }
    }
}
type Result<T> = std::result::Result<T, Error>;
fn refused() -> AdapterError {
    AdapterError::Rejected("Agent access tool authority does not match".into())
}
fn unavailable<T>(_: T) -> AdapterError {
    AdapterError::Unavailable("Agent access tool authority storage is unavailable".into())
}
