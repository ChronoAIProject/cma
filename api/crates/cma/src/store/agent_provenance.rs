use layer2_core::resource_access::{ResourceAccessStoreError, ResourceOwner, ResourceTarget};
use layer2_core::sandbox::store::{
    AgentAdmission, AgentProvenanceSnapshot, AgentProvenanceStore, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, AgentCreationProvenance, Sandbox};
use layer2_core::AgentId;
use mongodb::bson::{doc, Document};
use mongodb::ClientSession;

use super::resource_access::{read_execution_snapshot, read_inventory_policy, ReadError};
use super::sandbox_mongo::{
    is_transient, read_record, record_bson, start_transaction, MongoSandboxStore,
    DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};

mod snapshot;

pub(super) fn document(record: &AgentCreationProvenance) -> Result<Document, SandboxStoreError> {
    Ok(doc! {
        "_id": record.agent_id.as_str(), "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "record": record_bson(record).map_err(SandboxStoreError::Corrupt)?,
    })
}

pub(super) fn read(value: &Document) -> Result<AgentCreationProvenance, SandboxStoreError> {
    let record: AgentCreationProvenance = read_record(value).map_err(SandboxStoreError::Corrupt)?;
    if value.get_str("_id").ok() != Some(record.agent_id.as_str()) {
        return Err(SandboxStoreError::Corrupt(
            "Agent provenance index disagrees with identity".into(),
        ));
    }
    Ok(record)
}

pub(super) async fn record_creation(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    agent: &Agent,
    parent: &Sandbox,
    initiation: Option<&layer2_core::trigger_domain::LaunchInitiation>,
    session: &mut ClientSession,
) -> Result<AgentCreationProvenance, ReadError> {
    let policy = store
        .resource_policies
        .find_one(doc! { "_id": parent.id.as_str() })
        .session(&mut *session)
        .await?;
    let owner = match policy.as_ref().map(read_inventory_policy).transpose()? {
        Some(policy) => {
            policy.personal_owner_of(parent)?;
            policy.owner
        }
        None => ResourceOwner::Person {
            subject: parent.owner.clone(),
        },
    };
    let record = match initiation {
        Some(initiation) => AgentCreationProvenance::for_trigger_admission(
            admission, initiation, agent, parent, owner,
        ),
        None => AgentCreationProvenance::for_admission(
            agent,
            parent,
            owner,
            admission.created_by.as_ref(),
        ),
    }
    .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    store
        .agent_creation_provenance
        .insert_one(document(&record).map_err(|_| ResourceAccessStoreError::Corrupt)?)
        .session(session)
        .await?;
    Ok(record)
}

#[async_trait::async_trait]
impl AgentProvenanceStore for MongoSandboxStore {
    async fn agent_provenance_snapshot(
        &self,
        id: &AgentId,
    ) -> Result<AgentProvenanceSnapshot, SandboxStoreError> {
        for _ in 0..TRANSACTION_ATTEMPTS {
            let mut session = self
                .client
                .start_session()
                .await
                .map_err(|_| unavailable())?;
            start_transaction(&mut session)
                .await
                .map_err(|_| unavailable())?;
            let result = read_snapshot(self, id, &mut session).await;
            let _ = session.abort_transaction().await;
            match result {
                Ok(snapshot) => return Ok(snapshot),
                Err(ReadError::Driver(error)) if is_transient(&error) => continue,
                Err(ReadError::Driver(_)) => return Err(unavailable()),
                Err(ReadError::Domain(ResourceAccessStoreError::NotFound)) => {
                    return Err(SandboxStoreError::NotFound)
                }
                Err(ReadError::Domain(error)) => {
                    return Err(SandboxStoreError::Corrupt(error.to_string()))
                }
            }
        }
        Err(unavailable())
    }
}

fn unavailable() -> SandboxStoreError {
    SandboxStoreError::Unavailable("Agent provenance store is unavailable".into())
}

pub(super) async fn read_snapshot(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<AgentProvenanceSnapshot, ReadError> {
    match snapshot::read(store, id, session).await? {
        Some(snapshot) => Ok(snapshot),
        // Large retained records still use the original readers in the same
        // transaction; batching must not impose a new aggregate BSON limit.
        None => read_snapshot_sequential(store, id, session).await,
    }
}

async fn read_snapshot_sequential(
    store: &MongoSandboxStore,
    id: &AgentId,
    session: &mut ClientSession,
) -> Result<AgentProvenanceSnapshot, ReadError> {
    let resource = read_execution_snapshot(
        store,
        &ResourceTarget::Agent {
            agent_id: id.clone(),
        },
        session,
    )
    .await?;
    let creation = store
        .agent_creation_provenance
        .find_one(doc! { "_id": id.as_str() })
        .session(&mut *session)
        .await?
        .as_ref()
        .map(read)
        .transpose()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let first_log = store
        .agent_log
        .find_one(doc! { "agent_id": id.as_str(), "seq": 1_i64 })
        .session(&mut *session)
        .await?
        .as_ref()
        .map(super::agent_log::read_entry)
        .transpose()
        .map_err(|_| ResourceAccessStoreError::Corrupt)?;
    let snapshot = AgentProvenanceSnapshot {
        resource,
        creation,
        first_log,
    };
    // The aggregation may omit an oversized Agent. Recover its discriminator
    // in the same snapshot before trusting the separately fetched source row.
    let aggregate = store
        .agents
        .find_one(doc! {"_id":id.as_str()})
        .session(&mut *session)
        .await?
        .ok_or(ResourceAccessStoreError::Corrupt)?;
    verify_first_log(store, &aggregate, &snapshot, session).await?;
    Ok(snapshot)
}

async fn verify_first_log(
    store: &MongoSandboxStore,
    aggregate: &Document,
    snapshot: &AgentProvenanceSnapshot,
    session: &mut ClientSession,
) -> Result<(), ReadError> {
    use super::agent_log_integrity::TransactionLogError;
    use super::agent_log_segments as segments;

    if segments::read_format(aggregate)
        .map_err(|_| ResourceAccessStoreError::Corrupt)?
        .is_none()
    {
        return Ok(());
    }
    let agent =
        super::agents::read_agent(aggregate).map_err(|_| ResourceAccessStoreError::Corrupt)?;
    if snapshot.resource.lineage.agent.as_ref() != Some(&agent) {
        return Err(ResourceAccessStoreError::Corrupt.into());
    }
    let error = |error| match error {
        TransactionLogError::Driver(error) => ReadError::Driver(error),
        TransactionLogError::Domain(_) => ReadError::Domain(ResourceAccessStoreError::Corrupt),
    };
    let head = segments::read_head(store, aggregate, session, false)
        .await
        .map_err(error)?;
    let mut first = None;
    segments::visit_range(
        store,
        &head,
        segments::LogRange {
            after: 0,
            through: (head.last_seq as u64).min(1),
            limit: 1,
        },
        session,
        false,
        |entry| {
            first = Some(entry);
            Ok(())
        },
    )
    .await
    .map_err(error)?;
    if first != snapshot.first_log {
        return Err(ResourceAccessStoreError::Corrupt.into());
    }
    Ok(())
}
