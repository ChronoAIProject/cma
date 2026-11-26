//! Limited consent-request capabilities, separate from all NyxID credentials.
use crate::store::transaction_terminal::GuardedTerminal as _;
use std::sync::Arc;

use layer2_core::sandbox::{AgentCreationProvenance, RuntimeBinding};
use layer2_core::AgentId;
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

mod authenticator;
#[cfg(test)]
pub(crate) mod fixtures;
#[cfg(test)]
mod migration_tests;
mod operations;
mod origins;
mod records;
#[cfg(test)]
pub(in crate::store) mod tests;
mod transport;
#[cfg(test)]
mod transport_tests;
mod validation;
pub(super) use origins::AccessRecordAuthority;
use records::{OpenRecord, Scope};

pub(super) const COLLECTION: &str = "agent_access_tools";

/// This identity can request consent only. Owner approval remains a separate
/// authenticated command; never treat this as authority to change any grant.
pub(crate) use layer2_core::sandbox::agent_access_tool::AgentAccessRequester as AuthenticatedAgentAccess;

pub(crate) struct MongoAgentAccessToolSource {
    standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    store: Arc<MongoSandboxStore>,
    records: AccessRecordAuthority,
}

impl MongoAgentAccessToolSource {
    pub(crate) fn with_account_standing(
        mut self: Arc<Self>,
        standing: Arc<dyn layer2_core::account_deletion::AccountStanding>,
    ) -> Arc<Self> {
        Arc::get_mut(&mut self)
            .expect("tool standing configured before sharing")
            .standing = standing;
        self
    }
    async fn require_owner_available(&self, owner: &layer2_core::SubjectId) -> Result<()> {
        if self
            .standing
            .erasing(owner)
            .await
            .map_err(|_| Error::Unavailable)?
        {
            return Err(Error::Erasing);
        }
        Ok(())
    }
    #[cfg(test)]
    pub(crate) fn new(
        store: Arc<MongoSandboxStore>,
        public_origin: &str,
        encryption: &AuthEncryptionKey,
    ) -> std::result::Result<Arc<Self>, String> {
        Self::with_retained_origins(store, public_origin, &[], encryption)
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
        // Retained replay writes the same Agent while native tool setup is
        // pending. Join the recorder's local queue before opening a snapshot,
        // otherwise its steady progress can abort every confirmation attempt.
        // This is scheduling only: all lineage, lease and capability checks
        // remain in the transaction, including cross-replica write fences.
        let agent = match &operation {
            Operation::Resolve(_, request, _) => Some(AgentId(request.options.slot.0.clone())),
            Operation::Confirm(binding, _) | Operation::Transport(binding, ..) => {
                Some(AgentId(binding.scope().1 .0.clone()))
            }
            Operation::Authenticate(_) | Operation::Setup(_) => None,
        };
        let _writer = match agent {
            Some(id) => Some(self.store.agent_writes.lock(&id).await),
            None => None,
        };
        let jitter = uuid::Uuid::new_v4().as_u128() as u64;
        for attempt in 0..TRANSACTION_ATTEMPTS {
            // Only known-aborted transactions reach another attempt. Give a
            // concurrent lineage writer time to finish before rereading all
            // authority; unknown commits and refusals still return immediately.
            if attempt > 0 {
                let delay =
                    (100_u64 << (attempt - 1).min(3)) + (jitter.rotate_left(attempt as u32) & 31);
                tokio::time::sleep(std::time::Duration::from_millis(delay)).await;
            }
            let mut session = match self.store.client.start_session().await {
                Ok(session) => session,
                Err(error) => {
                    operation.diagnostic("start_session", driver_category(&error), attempt);
                    if is_transient(&error) {
                        continue;
                    }
                    return Err(unavailable(()));
                }
            };
            if let Err(error) = start_transaction(&mut session).await {
                operation.diagnostic("start_transaction", driver_category(&error), attempt);
                if is_transient(&error) {
                    continue;
                }
                return Err(unavailable(()));
            }
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
                Operation::Transport(binding, origin, conversation, sequence) => self
                    .transport_in(binding, origin, conversation, sequence, &mut session)
                    .await
                    .map(|binding| Outcome::Resolved(Some(binding))),
                Operation::Setup(capability) => self
                    .setup_in(capability, &mut session)
                    .await
                    .map(|()| Outcome::Confirmed),
            };
            let result = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    match error {
                        Error::Driver(error) => {
                            operation.diagnostic("read_write", driver_category(&error), attempt);
                            if !error
                                .contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT)
                                && (is_transient(&error) || is_duplicate_key(&error))
                            {
                                continue;
                            }
                            return Err(unavailable(()));
                        }
                        Error::Erasing => return Err(AdapterError::AccessRevoked),
                        Error::Refused => {
                            operation.diagnostic("read_write", "authority_rejected", attempt);
                            return Err(refused());
                        }
                        Error::Unavailable => {
                            operation.diagnostic("read_write", "storage_unavailable", attempt);
                            return Err(unavailable(()));
                        }
                    }
                }
            };
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(result),
                CommitOutcome::Retry(_) => {
                    operation.diagnostic("commit", "transient_transaction", attempt);
                    continue;
                }
                CommitOutcome::Unknown(_) => {
                    operation.diagnostic("commit", "unknown_commit", attempt);
                    return Err(unavailable(()));
                }
                CommitOutcome::Failed(_) => {
                    operation.diagnostic("commit", "driver_failure", attempt);
                    return Err(unavailable(()));
                }
            }
        }
        operation.diagnostic("retry", "retry_exhausted", TRANSACTION_ATTEMPTS - 1);
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

    async fn prepare_transport(
        &self,
        binding: &HostedAccessToolBinding,
        origin: &str,
        conversation: &str,
        operation_sequence: u64,
    ) -> layer3_adapter::Result<HostedAccessToolBinding> {
        match self
            .run(Operation::Transport(
                binding,
                Some(origin),
                conversation,
                operation_sequence,
            ))
            .await?
        {
            Outcome::Resolved(Some(binding)) => Ok(binding),
            _ => unreachable!("transport returns a binding"),
        }
    }

    async fn confirm_transport(
        &self,
        binding: &HostedAccessToolBinding,
        conversation: &str,
        operation_sequence: u64,
    ) -> layer3_adapter::Result<()> {
        self.run(Operation::Transport(
            binding,
            None,
            conversation,
            operation_sequence,
        ))
        .await?;
        Ok(())
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
    Transport(&'a HostedAccessToolBinding, Option<&'a str>, &'a str, u64),
    Authenticate(&'a str),
    Setup(&'a str),
}

impl Operation<'_> {
    fn diagnostic(&self, stage: &'static str, category: &'static str, attempt: usize) {
        let (operation, agent_id) = match self {
            Self::Resolve(_, request, _) => ("resolve", Some(request.options.slot.as_str())),
            Self::Confirm(binding, _) => ("confirm_open", Some(binding.scope().1.as_str())),
            Self::Transport(binding, _, _, _) => ("transport", Some(binding.scope().1.as_str())),
            Self::Authenticate(_) => ("authenticate", None),
            Self::Setup(_) => ("setup", None),
        };
        // Bearer-only operations have no safe Agent identity at this boundary.
        // Never log their input, bindings, driver text or stored documents.
        if let Some(agent_id) = agent_id {
            tracing::warn!(
                operation,
                stage,
                category,
                attempt = attempt + 1,
                agent_id,
                "Agent access tool authority transaction did not complete"
            );
        } else {
            tracing::warn!(
                operation,
                stage,
                category,
                attempt = attempt + 1,
                "Agent access tool authority transaction did not complete"
            );
        }
    }
}

fn driver_category(error: &mongodb::error::Error) -> &'static str {
    if error.contains_label(mongodb::error::UNKNOWN_TRANSACTION_COMMIT_RESULT) {
        "unknown_commit"
    } else if is_transient(error) {
        "transient_transaction"
    } else if is_duplicate_key(error) {
        "duplicate_key"
    } else {
        "driver_failure"
    }
}

enum Outcome {
    Resolved(Option<HostedAccessToolBinding>),
    Confirmed,
    Authenticated(Box<AuthenticatedAgentAccess>),
}
enum Error {
    Erasing,
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
impl From<super::agent_admission::helper_error::HelperError> for Error {
    fn from(value: super::agent_admission::helper_error::HelperError) -> Self {
        match value {
            super::agent_admission::helper_error::HelperError::Driver(error) => Self::Driver(error),
            super::agent_admission::helper_error::HelperError::Domain(_) => Self::Refused,
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
