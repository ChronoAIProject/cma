//! Runtime-writer claims serialized with the owning Sandbox capture marker.

use layer2_core::sandbox::store::SandboxStoreError;
use layer2_core::sandbox::{Agent, Workspace};
use layer2_core::{SandboxId, SubjectId};
use mongodb::bson::{doc, Document, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::{ClientSession, Collection};

use super::agent_log_integrity::carry_integrity_for_revision;
use super::agent_title_state::record_title_value;
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::read_sandbox;
use super::workspaces::{read_workspace, workspace_replacement_fields};

pub(super) enum ChildWriterClaim {
    Agent(Box<Agent>),
    Workspace(Box<Workspace>),
}

impl ChildWriterClaim {
    fn collection<'a>(&self, store: &'a MongoSandboxStore) -> &'a Collection<Document> {
        match self {
            Self::Agent(_) => &store.agents,
            Self::Workspace(_) => &store.workspaces,
        }
    }

    fn identity(&self) -> (&str, &SandboxId, &SubjectId, u64) {
        match self {
            Self::Agent(row) => (row.id.as_str(), &row.sandbox_id, &row.owner, row.revision),
            Self::Workspace(row) => (row.id.as_str(), &row.sandbox_id, &row.owner, row.revision),
        }
    }

    fn fields(&self) -> Result<Document, SandboxStoreError> {
        match self {
            Self::Agent(row) => agent_replacement_fields(row),
            Self::Workspace(row) => workspace_replacement_fields(row),
        }
    }

    fn matches(&self, document: &Document) -> Result<bool, SandboxStoreError> {
        match self {
            Self::Agent(expected) => read_agent(document).map(|row| row == **expected),
            Self::Workspace(expected) => read_workspace(document).map(|row| row == **expected),
        }
    }
}

enum ClaimError {
    Domain(SandboxStoreError),
    Driver(mongodb::error::Error),
}

pub(super) async fn persist(
    store: &MongoSandboxStore,
    expected_revision: u64,
    claim: ChildWriterClaim,
) -> Result<bool, SandboxStoreError> {
    if expected_revision.checked_add(1) != Some(claim.identity().3) {
        return Err(SandboxStoreError::Conflict);
    }
    let mut last_transient = None;
    for _ in 0..TRANSACTION_ATTEMPTS {
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(error.to_string());
                continue;
            }
            return Err(SandboxStoreError::Unavailable(error.to_string()));
        }
        match apply(store, expected_revision, &claim, &mut session).await {
            Ok(true) => {}
            Ok(false) => {
                let _ = session.abort_transaction().await;
                return Ok(false);
            }
            Err(ClaimError::Domain(error)) => {
                let _ = session.abort_transaction().await;
                return Err(error);
            }
            Err(ClaimError::Driver(error)) if is_transient(&error) => {
                let _ = session.abort_transaction().await;
                last_transient = Some(error.to_string());
                continue;
            }
            Err(ClaimError::Driver(error)) => {
                let _ = session.abort_transaction().await;
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(true),
            CommitOutcome::Retry(error) => last_transient = Some(error.to_string()),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    SandboxStoreError::Unavailable(format!(
                        "writer claim commit has no operation time: {error}"
                    ))
                })?;
                let store = store.clone();
                return wait_for_reconciliation("writer claim commit", async move {
                    if reconcile(&store, &claim, snapshot_time).await? {
                        Ok(true)
                    } else {
                        Err(SandboxStoreError::Unavailable(format!(
                            "writer claim commit remained unknown: {error}"
                        )))
                    }
                })
                .await
                .map_err(SandboxStoreError::Unavailable)?;
            }
            CommitOutcome::Failed(error) => {
                return Err(SandboxStoreError::Unavailable(error.to_string()));
            }
        }
    }
    Err(SandboxStoreError::Unavailable(format!(
        "writer claim exhausted its retry budget: {}",
        last_transient.unwrap_or_else(|| "no driver result".into())
    )))
}

async fn apply(
    store: &MongoSandboxStore,
    expected_revision: u64,
    claim: &ChildWriterClaim,
    session: &mut ClientSession,
) -> Result<bool, ClaimError> {
    let (id, sandbox_id, owner, _) = claim.identity();
    let parent = store
        .sandboxes
        .find_one(doc! { "_id": sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ClaimError::Driver)?
        .as_ref()
        .map(read_sandbox)
        .transpose()
        .map_err(ClaimError::Domain)?
        .ok_or(ClaimError::Domain(SandboxStoreError::NotFound))?;
    if parent.owner != *owner {
        return Err(ClaimError::Domain(SandboxStoreError::NotFound));
    }
    if parent.retirement_archive_pending()
        || parent.lifecycle.checkpoint_capture.is_some()
        || (parent.status == layer2_core::sandbox::SandboxStatus::Rotating
            && !matches!(claim, ChildWriterClaim::Agent(agent) if parent.restored_connection_is_ready(agent)))
    {
        return Ok(false);
    }
    if let ChildWriterClaim::Workspace(workspace) = claim {
        if parent.defers_lifecycle_workspace(workspace)
            || parent.status == layer2_core::sandbox::SandboxStatus::Rotating
        {
            return Ok(false);
        }
    }
    let class = match claim {
        ChildWriterClaim::Agent(_) => {
            layer2_core::sandbox::ledger_maintenance::LedgerWriterClass::AgentWorker
        }
        ChildWriterClaim::Workspace(_) => {
            layer2_core::sandbox::ledger_maintenance::LedgerWriterClass::WorkspaceWorker
        }
    };
    // The durable hold and the child lease share this transaction. Existing
    // ownership, lifecycle and worker revision checks remain mandatory.
    use super::ledger_maintenance::ApplyError as MaintenanceApplyError;
    use layer2_core::sandbox::store::ledger_maintenance::LedgerMaintenanceStoreError as MaintenanceError;
    match super::ledger_maintenance_writer::fence_writer(store, sandbox_id, class, None, session)
        .await
    {
        Ok(()) => {}
        Err(MaintenanceApplyError::Domain(MaintenanceError::WriterRefused(_))) => return Ok(false),
        Err(MaintenanceApplyError::Domain(MaintenanceError::Corrupt)) => {
            return Err(ClaimError::Domain(SandboxStoreError::Corrupt(
                "ledger maintenance authority is inconsistent".into(),
            )));
        }
        Err(MaintenanceApplyError::Domain(_)) => {
            return Err(ClaimError::Domain(SandboxStoreError::Unavailable(
                "ledger maintenance authority is unavailable".into(),
            )));
        }
        Err(MaintenanceApplyError::Driver(error)) => return Err(ClaimError::Driver(error)),
    }
    let parent_revision = signed(parent.revision, "writer claim parent revision")
        .map_err(|_| ClaimError::Domain(SandboxStoreError::Conflict))?;
    let locked = store
        .sandboxes
        .update_one(
            doc! {
                "_id": sandbox_id.as_str(),
                "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                "revision": parent_revision,
            },
            doc! { "$inc": { "runtime_writer_claim_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(ClaimError::Driver)?;
    if locked.matched_count != 1 {
        return Err(ClaimError::Domain(SandboxStoreError::Conflict));
    }
    let revision = signed(expected_revision, "writer claim revision")
        .map_err(|_| ClaimError::Domain(SandboxStoreError::Conflict))?;
    let current_agent_row = if matches!(claim, ChildWriterClaim::Agent(_)) {
        Some(
            store
                .agents
                .find_one(doc! { "_id": id })
                .session(&mut *session)
                .await
                .map_err(ClaimError::Driver)?
                .ok_or(ClaimError::Domain(SandboxStoreError::NotFound))?,
        )
    } else {
        None
    };
    let mut filter = doc! {
        "_id": id,
        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
        "revision": revision,
        "channel_reuse_cleanup_execution": { "$exists": false },
                "owned_workspace_cleanup_admission_v1": { "$exists": false },
    };
    if let ChildWriterClaim::Agent(agent) = claim {
        filter.insert(
            "last_seq",
            signed(agent.conversation.last_seq, "writer claim Agent log head")
                .map_err(|error| ClaimError::Domain(SandboxStoreError::Corrupt(error)))?,
        );
        filter.insert("record.title", record_title_value(agent));
    }
    let mut fields = claim.fields().map_err(ClaimError::Domain)?;
    if let (ChildWriterClaim::Agent(agent), Some(current_row)) = (claim, current_agent_row.as_ref())
    {
        carry_integrity_for_revision(
            current_row,
            &read_agent(current_row).map_err(ClaimError::Domain)?,
            agent,
            &mut fields,
        )
        .map_err(ClaimError::Domain)?;
    }
    let advanced = claim
        .collection(store)
        .update_one(filter, doc! { "$set": fields })
        .session(session)
        .await
        .map_err(ClaimError::Driver)?;
    if advanced.matched_count == 1 {
        Ok(true)
    } else {
        Err(ClaimError::Domain(SandboxStoreError::Conflict))
    }
}

async fn reconcile(
    store: &MongoSandboxStore,
    claim: &ChildWriterClaim,
    snapshot_time: Timestamp,
) -> Result<bool, SandboxStoreError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    let document = claim
        .collection(store)
        .find_one(doc! { "_id": claim.identity().0 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| SandboxStoreError::Unavailable(error.to_string()))?;
    document
        .as_ref()
        .map_or(Ok(false), |document| claim.matches(document))
}
