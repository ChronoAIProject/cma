//! Runtime-writer claims serialized with the owning Sandbox capture marker.

use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::store::{
    AgentWorkClaimDetail as ClaimDetail, AgentWorkClaimError, SandboxStoreError,
};
use layer2_core::sandbox::{Agent, Workspace};
use layer2_core::{SandboxId, SubjectId};
use mongodb::bson::{doc, Document, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::{ClientSession, Collection};

use super::agent_capacity_recovery::source::envelope_filter;
use super::agent_log_integrity::carry_integrity_for_revision;
use super::agent_title_state::record_title_value;
use super::agents::{agent_replacement_fields, read_agent};
use super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::sandboxes::read_sandbox;
use super::work_claim_authority::LEDGER_MAINTENANCE_FIELD;
use super::workspaces::{read_workspace, workspace_replacement_fields};

mod diagnostics;

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
    Domain(AgentWorkClaimError),
    Driver(mongodb::error::Error, ClaimDetail),
}

impl ClaimError {
    fn domain(error: SandboxStoreError) -> Self {
        Self::Domain(error.into())
    }

    fn driver(error: mongodb::error::Error) -> Self {
        Self::Driver(error, ClaimDetail::Driver)
    }

    fn read(error: mongodb::error::Error) -> Self {
        Self::Driver(error, ClaimDetail::Read)
    }
}

fn unavailable(error: impl std::fmt::Display, detail: ClaimDetail) -> AgentWorkClaimError {
    AgentWorkClaimError::with_detail(SandboxStoreError::Unavailable(error.to_string()), detail)
}

pub(super) async fn persist(
    store: &MongoSandboxStore,
    expected_revision: u64,
    claim: ChildWriterClaim,
) -> Result<bool, SandboxStoreError> {
    persist_observed(store, expected_revision, claim)
        .await
        .map_err(|error| error.store_error)
}

pub(super) async fn persist_observed(
    store: &MongoSandboxStore,
    expected_revision: u64,
    claim: ChildWriterClaim,
) -> Result<bool, AgentWorkClaimError> {
    if expected_revision.checked_add(1) != Some(claim.identity().3) {
        return Err(SandboxStoreError::Conflict.into());
    }
    let mut last_transient = None;
    let mut diagnostics = diagnostics::ClaimDiagnostics::new(&claim);
    for attempt in 0..TRANSACTION_ATTEMPTS {
        // The previous session has been aborted/dropped before this delay.
        // Keep exhaustion Unavailable: outer claim loops retry domain conflicts
        // and must not multiply this known-abort transaction budget.
        super::contention::backoff(attempt).await;
        diagnostics.attempt(attempt + 1);
        let mut session = store
            .client
            .start_session()
            .await
            .map_err(|error| unavailable(error, ClaimDetail::Driver))?;
        if let Err(error) = start_transaction(&mut session).await {
            if is_transient(&error) {
                last_transient = Some(diagnostics.transient(&error));
                continue;
            }
            return Err(unavailable(error, ClaimDetail::Driver));
        }
        match apply(
            store,
            expected_revision,
            &claim,
            &mut session,
            &mut diagnostics,
        )
        .await
        {
            Ok(true) => {}
            Ok(false) => {
                let _ = session.abort_guarded().await;
                return Ok(false);
            }
            Err(ClaimError::Domain(error)) => {
                let _ = session.abort_guarded().await;
                return Err(error);
            }
            Err(ClaimError::Driver(error, _)) if is_transient(&error) => {
                let _ = session.abort_guarded().await;
                last_transient = Some(diagnostics.transient(&error));
                continue;
            }
            Err(ClaimError::Driver(error, detail)) => {
                let _ = session.abort_guarded().await;
                return Err(unavailable(error, detail));
            }
        }
        diagnostics.phase = "commit";
        match commit_transaction(&mut session).await {
            CommitOutcome::Committed => return Ok(true),
            CommitOutcome::Retry(error) => last_transient = Some(diagnostics.transient(&error)),
            CommitOutcome::Unknown(error) => {
                let snapshot_time = session.operation_time().ok_or_else(|| {
                    unavailable(
                        format!("writer claim commit has no operation time: {error}"),
                        ClaimDetail::Driver,
                    )
                })?;
                let store = store.clone();
                return wait_for_reconciliation("writer claim commit", async move {
                    if reconcile(&store, &claim, snapshot_time).await? {
                        Ok(true)
                    } else {
                        Err(unavailable(
                            format!("writer claim commit remained unknown: {error}"),
                            ClaimDetail::Driver,
                        ))
                    }
                })
                .await
                .map_err(|error| unavailable(error, ClaimDetail::Driver))?;
            }
            CommitOutcome::Failed(error) => {
                return Err(unavailable(error, ClaimDetail::Driver));
            }
        }
    }
    Err(unavailable(
        format!(
            "writer claim exhausted its retry budget: {}",
            last_transient.unwrap_or_else(|| "no driver result".into())
        ),
        ClaimDetail::RetryExhausted,
    ))
}

async fn apply(
    store: &MongoSandboxStore,
    expected_revision: u64,
    claim: &ChildWriterClaim,
    session: &mut ClientSession,
    diagnostics: &mut diagnostics::ClaimDiagnostics,
) -> Result<bool, ClaimError> {
    if let ChildWriterClaim::Agent(agent) = claim {
        use super::agent_ownership::{FenceError, ObservedFenceError};
        store
            .fence_agent_effect_observed(&agent.id, session)
            .await
            .map_err(|error| match error {
                ObservedFenceError::Read(error) => ClaimError::read(error),
                ObservedFenceError::Fence(FenceError::Domain(error)) => ClaimError::domain(error),
                ObservedFenceError::Fence(FenceError::Driver(error)) => ClaimError::driver(error),
            })?;
    }
    let (id, sandbox_id, owner, _) = claim.identity();
    crate::store::work_publication::fence_transaction(store, session)
        .await
        .map_err(|error| match error {
            super::work_publication::FenceError::Domain(error) => ClaimError::domain(error),
            super::work_publication::FenceError::Driver(error) => ClaimError::driver(error),
        })?;
    diagnostics.phase = "parent_read";
    let parent_row = store
        .sandboxes
        .find_one(doc! { "_id": sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(ClaimError::read)?
        .ok_or(ClaimError::domain(SandboxStoreError::NotFound))?;
    let parent = read_sandbox(&parent_row).map_err(ClaimError::domain)?;
    if parent.owner != *owner {
        return Err(ClaimError::domain(SandboxStoreError::NotFound));
    }
    if parent_row.contains_key(LEDGER_MAINTENANCE_FIELD)
        || parent.retirement_archive_pending()
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
    let parent_revision = signed(parent.revision, "writer claim parent revision")
        .map_err(|_| ClaimError::domain(SandboxStoreError::Conflict))?;
    diagnostics.phase = "parent_fence";
    let locked = store
        .sandboxes
        .update_one(
            crate::store::work_schema::query(
                crate::store::work_publication::sandbox(
                    doc! {
                        "_id": sandbox_id.as_str(),
                        "document_schema_version": DOCUMENT_SCHEMA_VERSION,
                        "revision": parent_revision,
                    },
                    sandbox_id.as_str(),
                )
                .map_err(ClaimError::domain)?,
            ),
            doc! { "$inc": { "runtime_writer_claim_fence": 1_i64 } },
        )
        .session(&mut *session)
        .await
        .map_err(ClaimError::driver)?;
    if locked.matched_count != 1 {
        return Err(ClaimError::domain(SandboxStoreError::Conflict));
    }
    let revision = signed(expected_revision, "writer claim revision")
        .map_err(|_| ClaimError::domain(SandboxStoreError::Conflict))?;
    diagnostics.phase = "child_read";
    let current_agent_row = if matches!(claim, ChildWriterClaim::Agent(_)) {
        Some(
            store
                .agents
                .find_one(doc! { "_id": id })
                .session(&mut *session)
                .await
                .map_err(ClaimError::read)?
                .ok_or(ClaimError::domain(SandboxStoreError::NotFound))?,
        )
    } else {
        None
    };
    use super::agent_inspection_park::{self, ClaimPark};
    let mut consumed_park = None;
    let mut filter = if let (ChildWriterClaim::Agent(agent), Some(current_row)) =
        (claim, current_agent_row.as_ref())
    {
        let current = read_agent(current_row).map_err(ClaimError::domain)?;
        if !super::agent_worker_transaction::pending_adoption::witness::fence_replacement(
            store,
            current_row,
            &current,
            &parent,
            session,
        )
        .await
        .map_err(|error| match error {
            super::agent_log_integrity::TransactionLogError::Domain(error) => ClaimError::Domain(
                AgentWorkClaimError::with_detail(error, ClaimDetail::PublicationCustody),
            ),
            super::agent_log_integrity::TransactionLogError::Driver(error) => {
                ClaimError::Driver(error, ClaimDetail::PublicationCustody)
            }
        })? {
            return Ok(false);
        }
        if store.ownership_retry_hints_active() {
            use super::agent_failure_pacing::ownership;
            if let Some((row, now)) = ownership::server_row(store, &current.id, session)
                .await
                .map_err(|error| {
                    ClaimError::Domain(AgentWorkClaimError::with_detail(error, ClaimDetail::Read))
                })?
            {
                if ownership::holds(&row, &current, &parent, now) {
                    return Ok(false);
                }
            }
        }
        if current.revision != expected_revision
            || current.conversation.last_seq != agent.conversation.last_seq
        {
            return Err(ClaimError::domain(SandboxStoreError::Conflict));
        }
        match agent_inspection_park::for_claim(
            store,
            current_row,
            &current,
            &parent,
            agent,
            session,
        )
        .await
        .map_err(ClaimError::domain)?
        {
            ClaimPark::Hold => return Ok(false),
            ClaimPark::Proceed => {}
            ClaimPark::Retry(marker) => consumed_park = Some(marker),
        }
        diagnostics.phase = "child_validation";
        super::agent_log_segments::ordinary::source(store, &current, current_row, session)
            .await
            .map_err(|error| match error {
                super::agent_log_integrity::TransactionLogError::Domain(error) => {
                    ClaimError::domain(error)
                }
                super::agent_log_integrity::TransactionLogError::Driver(error) => {
                    ClaimError::read(error)
                }
            })?;
        envelope_filter(&current, current_row).map_err(ClaimError::domain)?
    } else {
        doc! {
            "_id": id,
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "revision": revision,
            "channel_reuse_cleanup_execution": { "$exists": false },
            "owned_workspace_cleanup_admission_v1": { "$exists": false },
        }
    };
    filter.insert("channel_reuse_cleanup_execution", doc! { "$exists": false });
    filter.insert(
        "owned_workspace_cleanup_admission_v1",
        doc! { "$exists": false },
    );
    if let ChildWriterClaim::Agent(agent) = claim {
        let pending = &super::agent_worker_transaction::pending_adoption::witness::FIELD;
        if super::agent_worker_transaction::pending_adoption::witness::pending_step(agent) {
            if let Some(marker) = current_agent_row.as_ref().and_then(|row| row.get(*pending)) {
                filter.insert(*pending, marker.clone());
            } else {
                filter.insert(*pending, doc! {"$exists": false});
            }
        }
        filter.insert(
            "last_seq",
            signed(agent.conversation.last_seq, "writer claim Agent log head")
                .map_err(|error| ClaimError::domain(SandboxStoreError::Corrupt(error)))?,
        );
        filter.insert("record.title", record_title_value(agent));
    }
    if let ChildWriterClaim::Workspace(workspace) = claim {
        let lease = workspace
            .work
            .lease
            .as_ref()
            .ok_or(ClaimError::domain(SandboxStoreError::Conflict))?;
        let acquisition =
            super::work_claim_authority::eligible(&lease.expires_at).map_err(ClaimError::domain)?;
        filter = super::work_schema::query(doc! {"$and":[filter, acquisition]});
    }
    let mut fields = claim.fields().map_err(ClaimError::domain)?;
    if store.agent_inspection_parking && matches!(claim, ChildWriterClaim::Agent(_)) {
        let proof = if let Some(marker) = consumed_park {
            let get = |field| {
                marker.get(field).cloned().ok_or_else(|| {
                    ClaimError::domain(SandboxStoreError::Corrupt(
                        "inspection claim proof missing".into(),
                    ))
                })
            };
            let proof = doc! {"id": get("id")?, "binding": get("binding")?};
            fields.insert(agent_inspection_park::FIELD, marker);
            mongodb::bson::Bson::Document(proof)
        } else {
            mongodb::bson::Bson::Null
        };
        fields.insert(agent_inspection_park::CLAIM_PROOF, proof);
    }
    if let ChildWriterClaim::Agent(agent) = claim {
        store
            .agent_work_owner_fields(&agent.id, &mut fields)
            .map_err(ClaimError::domain)?;
    }
    if let (ChildWriterClaim::Agent(agent), Some(current_row)) = (claim, current_agent_row.as_ref())
    {
        carry_integrity_for_revision(
            current_row,
            &read_agent(current_row).map_err(ClaimError::domain)?,
            agent,
            &mut fields,
        )
        .map_err(ClaimError::domain)?;
    }
    if matches!(claim, ChildWriterClaim::Workspace(row) if row.work.authority_protocol.is_some()) {
        super::work_schema::floor::retain(store, session)
            .await
            .map_err(ClaimError::driver)?;
    }
    diagnostics.phase = "child_write";
    let update: mongodb::options::UpdateModifications = match claim {
        ChildWriterClaim::Workspace(_) => super::work_lease_update::acquisition(fields).into(),
        ChildWriterClaim::Agent(_) => doc! {"$set":fields}.into(),
    };
    let advanced = claim
        .collection(store)
        .update_one(filter, update)
        .session(session)
        .await
        .map_err(ClaimError::driver)?;
    if advanced.matched_count == 1 {
        Ok(true)
    } else {
        Err(ClaimError::domain(SandboxStoreError::Conflict))
    }
}

async fn reconcile(
    store: &MongoSandboxStore,
    claim: &ChildWriterClaim,
    snapshot_time: Timestamp,
) -> Result<bool, AgentWorkClaimError> {
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(snapshot_time)
        .await
        .map_err(|error| unavailable(error, ClaimDetail::Driver))?;
    let document = claim
        .collection(store)
        .find_one(doc! { "_id": claim.identity().0 })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|error| unavailable(error, ClaimDetail::Read))?;
    document
        .as_ref()
        .map_or(Ok(false), |document| claim.matches(document))
        .map_err(Into::into)
}
