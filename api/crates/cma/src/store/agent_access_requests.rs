//! Durable owner consent, original-conversation identity and leased execution.
#[cfg(test)]
use super::sandbox_mongo::is_transient;
use super::sandbox_mongo::{
    commit_transaction, is_confirmed_body_retry, start_transaction, CommitOutcome,
    MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_access::{validate_access_services, AgentAccessError};
use layer2_core::sandbox::agent_access_tool::{AgentAccessProposal, AgentAccessRequester};
use layer2_core::sandbox::agent_io::AgentAuthorizationRevision;
use layer2_core::{AgentId, ServiceId, SubjectId};
use mongodb::{bson::doc, ClientSession};

pub(crate) const COLLECTION: &str = "agent_access_requests";
const ACTIVE: [&str; 8] = [
    "pending",
    "connecting",
    "waiting_quiet",
    "updating",
    "pausing",
    "granting",
    "reviving",
    "notifying",
];
const APPLYING: [&str; 6] = [
    "waiting_quiet",
    "updating",
    "pausing",
    "granting",
    "reviving",
    "notifying",
];

pub(in crate::store) enum Error {
    Domain(AgentAccessError),
    Driver(mongodb::error::Error),
}
impl From<AgentAccessError> for Error {
    fn from(value: AgentAccessError) -> Self {
        Self::Domain(value)
    }
}
impl From<mongodb::error::Error> for Error {
    fn from(value: mongodb::error::Error) -> Self {
        Self::Driver(value)
    }
}
type TxResult<T> = Result<T, Error>;

fn provenance_error(error: super::resource_access::ReadError) -> Error {
    match error {
        super::resource_access::ReadError::Driver(error) => Error::Driver(error),
        _ => Error::Domain(AgentAccessError::NotFound),
    }
}

macro_rules! transaction {
    ($store:expr, $session:ident, $body:block) => {{
        let store = $store;
        let mut outcome = Err(AgentAccessError::Unavailable);
        for attempt in 0..TRANSACTION_ATTEMPTS {
            crate::store::contention::backoff(attempt).await;
            // A positively known abort at session start or the start fence
            // retries within the budget; any other failure stays unavailable.
            let mut owned_session = match store.client.start_session().await {
                Ok(session) => session,
                Err(error) if crate::store::sandbox_mongo::is_transient(&error) => continue,
                Err(_) => break,
            };
            if let Err(error) = start_transaction(&mut owned_session).await {
                if crate::store::sandbox_mongo::is_transient(&error) {
                    continue;
                }
                break;
            }
            let $session = &mut owned_session;
            let result: TxResult<_> = async { $body }.await;
            let value = match result {
                Ok(value) => value,
                Err(error) => {
                    let _ = owned_session.abort_guarded().await;
                    match error {
                        Error::Driver(error) if is_confirmed_body_retry(&error) => continue,
                        Error::Driver(_) => {
                            outcome = Err(AgentAccessError::Unavailable);
                            break;
                        }
                        Error::Domain(error) => {
                            outcome = Err(error);
                            break;
                        }
                    }
                }
            };
            match commit_transaction(&mut owned_session).await {
                CommitOutcome::Committed => {
                    outcome = Ok(value);
                    break;
                }
                CommitOutcome::Retry(_) => continue,
                _ => {
                    outcome = Err(AgentAccessError::Unavailable);
                    break;
                }
            }
        }
        outcome
    }};
}

mod authority;
mod records;
use authority::valid_owner_authorization;
pub(in crate::store) use records::{document, lease_filter, read};
pub(crate) use records::{
    AgentAccessDecision, AgentAccessHotUpdate, AgentAccessLease, AgentAccessNotificationIntent,
    AgentAccessPhase, AgentAccessProgress, AgentAccessRecord, AgentAccessScopeMode,
    ConnectionApprovalBinding,
};
mod lineage;
pub(in crate::store) use lineage::validate_lineage_transaction;
mod admission;
#[cfg(test)]
mod admission_retry_contract;
mod decisions;
pub(in crate::store) mod grant;
mod hot_grant;
mod notification;
mod owner_scope;
mod retry;
#[cfg(test)]
mod retry_tests;
#[cfg(test)]
pub(crate) mod tests;
mod worker;

pub(super) async fn ensure_indexes(
    store: &MongoSandboxStore,
) -> Result<(), layer2_core::StoreError> {
    for keys in [
        doc! {"record.requester.agent_id":1,"record.created_at":-1},
        doc! {"record.requester.agent_id":1,"record.proposal.retry_of":1},
        doc! {"record.requester.sandbox_id":1,"record.phase":1},
        doc! {"record.phase":1,"record.retry_at":1,"record.lease.expires_at":1},
    ] {
        store
            .agent_access_requests
            .create_index(mongodb::IndexModel::builder().keys(keys).build())
            .await
            .map_err(|_| {
                layer2_core::StoreError::Unavailable(
                    "Agent access request index unavailable".into(),
                )
            })?;
    }
    Ok(())
}

pub(in crate::store) fn now_millis() -> i64 {
    mongodb::bson::DateTime::now().timestamp_millis()
}
pub(super) fn command_hash(key: &str) -> Result<String, AgentAccessError> {
    if key.is_empty() || key.len() > 128 || key.trim() != key || key.chars().any(char::is_control) {
        return Err(AgentAccessError::Invalid);
    }
    records::digest(&("agent-access-decision-v1", key))
}
async fn fence(
    store: &MongoSandboxStore,
    snapshot: &layer2_core::sandbox::store::AgentProvenanceSnapshot,
    session: &mut ClientSession,
) -> TxResult<()> {
    super::resource_access::fence_lineage(store, &snapshot.resource, session)
        .await
        .map_err(|error| match error {
            super::resource_access::ReadError::Driver(error) => Error::Driver(error),
            _ => Error::Domain(AgentAccessError::Conflict),
        })
}
async fn replace(
    store: &MongoSandboxStore,
    previous_revision: u64,
    record: &AgentAccessRecord,
    session: &mut ClientSession,
) -> TxResult<()> {
    let revision = i64::try_from(previous_revision).map_err(|_| AgentAccessError::Unavailable)?;
    let result = store
        .agent_access_requests
        .replace_one(
            doc! {"_id": &record.id,"record.revision":revision,
            "document_schema_version":DOCUMENT_SCHEMA_VERSION},
            document(record)?,
        )
        .session(session)
        .await?;
    if result.matched_count != 1 {
        return Err(AgentAccessError::Conflict.into());
    }
    Ok(())
}

async fn replace_leased(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    previous_revision: u64,
    record: &AgentAccessRecord,
    session: &mut ClientSession,
) -> TxResult<()> {
    let mut predicate = lease_filter(lease, now_millis());
    predicate.insert(
        "record.revision",
        i64::try_from(previous_revision).map_err(|_| AgentAccessError::Unavailable)?,
    );
    if store
        .agent_access_requests
        .replace_one(predicate, document(record)?)
        .session(session)
        .await?
        .matched_count
        != 1
    {
        return Err(AgentAccessError::Conflict.into());
    }
    Ok(())
}
