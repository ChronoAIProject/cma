//! Admit owner-approved quiescence and its workflow receipt in one transaction.
use crate::store::transaction_terminal::GuardedTerminal as _;
use layer2_core::sandbox::agent_access::AgentAccessError;
use layer2_core::sandbox::store::{
    apply_sandbox_lifecycle_command, SandboxLifecycleCommand, SandboxLifecycleCommandApplication,
    SandboxMaintenanceOutcome, SandboxStoreError,
};
use layer2_core::sandbox::{
    Sandbox, SandboxLifecycleOperation, SandboxReviveCause, SandboxStatus, SandboxWorkStep,
};
use mongodb::bson::{doc, DateTime, Timestamp};
use mongodb::options::ReadConcern;
use mongodb::ClientSession;

use super::super::agent_access_requests::{
    self as access, AgentAccessDecision, AgentAccessLease, AgentAccessPhase, AgentAccessRecord,
};
use super::super::lifecycle_execution::load_sandbox_agents;
use super::super::sandbox_events::{append_in_transaction, validate_in_transaction};
use super::super::sandbox_mongo::{
    commit_transaction, is_transient, signed, start_transaction, unsigned, wait_for_reconciliation,
    CommitOutcome, MongoSandboxStore, DOCUMENT_SCHEMA_VERSION, TRANSACTION_ATTEMPTS,
};
use super::super::sandboxes::{read_sandbox, sandbox_replacement_fields};
use super::evidence::fence_live_agents;
#[cfg(test)]
use super::evidence::live_activities;
use super::transaction::{ApplyError, MaintenanceReceipt};
#[cfg(test)]
use layer2_core::sandbox::store::{apply_consented_quiet_pause, SandboxMaintenanceApplication};

struct Receipt {
    maintenance: MaintenanceReceipt,
    access: AgentAccessRecord,
}
#[derive(Debug)]
enum Error {
    Access(AgentAccessError),
    Maintenance(ApplyError),
}
impl From<AgentAccessError> for Error {
    fn from(error: AgentAccessError) -> Self {
        Self::Access(error)
    }
}
impl From<ApplyError> for Error {
    fn from(error: ApplyError) -> Self {
        Self::Maintenance(error)
    }
}
impl Error {
    fn transient(&self) -> bool {
        matches!(self, Self::Maintenance(ApplyError::Driver(error)) if is_transient(error))
    }
    fn public(self) -> AgentAccessError {
        match self {
            Self::Access(error) => error,
            Self::Maintenance(ApplyError::Domain(SandboxStoreError::NotFound)) => {
                AgentAccessError::NotFound
            }
            Self::Maintenance(ApplyError::Domain(
                SandboxStoreError::Conflict | SandboxStoreError::SequenceConflict,
            )) => AgentAccessError::Conflict,
            _ => AgentAccessError::Unavailable,
        }
    }
}
fn driver(error: mongodb::error::Error) -> Error {
    ApplyError::Driver(error).into()
}
fn domain(error: SandboxStoreError) -> Error {
    ApplyError::Domain(error).into()
}

#[derive(Clone, Copy)]
enum Action {
    #[cfg(test)]
    Pause,
    Revive,
}

impl MongoSandboxStore {
    #[cfg(test)]
    /// Seed historical paused workflows for recovery tests; new approvals update live.
    /// A consented pause never interrupts active/queued Agent work and cannot
    /// outlive the exact durable access-request lease that authorizes it.
    pub(crate) async fn schedule_access_request_pause(
        &self,
        lease: &AgentAccessLease,
    ) -> Result<SandboxMaintenanceOutcome, AgentAccessError> {
        self.schedule_access_lifecycle(lease, Action::Pause).await
    }

    /// Revival is admitted only for the exact completed pause of this request.
    pub(crate) async fn schedule_access_request_revival(
        &self,
        lease: &AgentAccessLease,
    ) -> Result<SandboxMaintenanceOutcome, AgentAccessError> {
        self.schedule_access_lifecycle(lease, Action::Revive).await
    }

    async fn schedule_access_lifecycle(
        &self,
        lease: &AgentAccessLease,
        action: Action,
    ) -> Result<SandboxMaintenanceOutcome, AgentAccessError> {
        for attempt in 0..TRANSACTION_ATTEMPTS {
            if attempt > 0 {
                tokio::task::yield_now().await;
            }
            let mut session = match self.client.start_session().await {
                Ok(session) => session,
                Err(error) if is_transient(&error) => continue,
                Err(_) => return Err(AgentAccessError::Unavailable),
            };
            if let Err(error) = start_transaction(&mut session).await {
                if is_transient(&error) {
                    continue;
                }
                return Err(AgentAccessError::Unavailable);
            }
            let receipt = match apply(self, lease, action, &mut session).await {
                Ok(receipt) => receipt,
                Err(error) => {
                    let _ = session.abort_guarded().await;
                    if error.transient() {
                        continue;
                    }
                    return Err(error.public());
                }
            };
            if receipt.maintenance.event.is_none() {
                let _ = session.abort_guarded().await;
                return Ok(receipt.maintenance.outcome);
            }
            match commit_transaction(&mut session).await {
                CommitOutcome::Committed => return Ok(receipt.maintenance.outcome),
                CommitOutcome::Retry(_) => continue,
                CommitOutcome::Failed(_) => return Err(AgentAccessError::Unavailable),
                CommitOutcome::Unknown(_) => {
                    let snapshot_time = session
                        .operation_time()
                        .ok_or(AgentAccessError::Unavailable)?;
                    let store = self.clone();
                    return wait_for_reconciliation(
                        "consented Sandbox lifecycle commit",
                        async move {
                            if reconcile(&store, &receipt, snapshot_time).await? {
                                Ok(receipt.maintenance.outcome)
                            } else {
                                Err(AgentAccessError::Unavailable)
                            }
                        },
                    )
                    .await
                    .map_err(|_| AgentAccessError::Unavailable)?;
                }
            }
        }
        Err(AgentAccessError::Unavailable)
    }
}

async fn apply(
    store: &MongoSandboxStore,
    lease: &AgentAccessLease,
    action: Action,
    session: &mut ClientSession,
) -> Result<Receipt, Error> {
    let document = store
        .agent_access_requests
        .find_one(access::lease_filter(
            lease,
            DateTime::now().timestamp_millis(),
        ))
        .session(&mut *session)
        .await
        .map_err(driver)?
        .ok_or(AgentAccessError::Conflict)?;
    let mut record = access::read(&document)?;
    if !matches!(record.decision, Some(AgentAccessDecision::Approved { .. }))
        || record.approved_at.is_none()
        || !match action {
            #[cfg(test)]
            Action::Pause => matches!(
                record.phase,
                AgentAccessPhase::WaitingQuiet | AgentAccessPhase::Pausing
            ),
            Action::Revive => record.phase == AgentAccessPhase::Reviving,
        }
    {
        return Err(AgentAccessError::Conflict.into());
    }
    let lineage = access::validate_lineage_transaction(store, &record.requester, session, false)
        .await
        .map_err(|error| match error {
            access::Error::Domain(error) => Error::Access(error),
            access::Error::Driver(error) => driver(error),
        })?;
    let document = store
        .sandboxes
        .find_one(doc! { "_id": record.requester.sandbox_id.as_str() })
        .session(&mut *session)
        .await
        .map_err(driver)?
        .ok_or(AgentAccessError::NotFound)?;
    let current = read_sandbox(&document).map_err(domain)?;
    let complete = |access: AgentAccessRecord, outcome| Receipt {
        maintenance: MaintenanceReceipt {
            sandbox: current.clone(),
            event: None,
            outcome,
            refusal: None,
            unproven_logs: Vec::new(),
        },
        access,
    };
    let recorded_operation = match action {
        #[cfg(test)]
        Action::Pause if record.phase == AgentAccessPhase::Pausing => Some(
            record
                .progress
                .pause_operation_id
                .as_deref()
                .ok_or(AgentAccessError::Unavailable)?,
        ),
        Action::Revive => record.progress.revive_operation_id.as_deref(),
        #[cfg(test)]
        Action::Pause => None,
    };
    if let Some(operation) = recorded_operation {
        let expected_status = match action {
            #[cfg(test)]
            Action::Pause => SandboxStatus::Paused,
            Action::Revive => SandboxStatus::Active,
        };
        let matching = current.lifecycle.operation.as_ref().map_or_else(
            || current.status == expected_status && latest_operation_matches(&current, operation),
            |active| {
                active.authority().operation_id == operation
                    && match action {
                        #[cfg(test)]
                        Action::Pause => matches!(active, SandboxLifecycleOperation::Pause { .. }),
                        Action::Revive => {
                            matches!(active, SandboxLifecycleOperation::Revive { .. })
                        }
                    }
            },
        );
        if !matching {
            return Err(AgentAccessError::Conflict.into());
        }
        return Ok(complete(
            record,
            SandboxMaintenanceOutcome::AlreadyScheduled,
        ));
    }
    let last_seq = document
        .get_i64("last_seq")
        .map_err(|_| AgentAccessError::Unavailable)
        .and_then(|value| {
            unsigned(value, "consented pause event sequence")
                .map_err(|_| AgentAccessError::Unavailable)
        })?;
    let agents = load_sandbox_agents(store, &current.id, session)
        .await
        .map_err(ApplyError::from)?;
    let requested_at = layer2_core::now_rfc3339();
    let (expected_revision, sandbox, event) = match action {
        #[cfg(test)]
        Action::Pause => {
            // Another approved request may have expanded this shared Sandbox
            // while this one waited. Reject stale consent before downtime;
            // the later grant must not discover it after pausing the runtime.
            let approved = record.approved_services()?;
            if current
                .credential
                .effective_reaches()
                .iter()
                .any(|service| !approved.contains(service))
            {
                return Err(AgentAccessError::Invalid.into());
            }
            let activities = live_activities(store, &agents, session, &requested_at).await?;
            match apply_consented_quiet_pause(
                &current,
                &activities,
                &record.requester.owner,
                &requested_at,
            )
            .map_err(domain)?
            {
                SandboxMaintenanceApplication::Complete(outcome) => {
                    return Ok(complete(record, outcome))
                }
                SandboxMaintenanceApplication::Persist {
                    expected_revision,
                    sandbox,
                    event,
                } => (expected_revision, *sandbox, event),
            }
        }
        Action::Revive => {
            let pause = record
                .progress
                .pause_operation_id
                .as_deref()
                .ok_or(AgentAccessError::Unavailable)?;
            if current.status != SandboxStatus::Paused
                || !latest_operation_matches(&current, pause)
                || current.lifecycle.operation.is_some()
                || current.lifecycle.checkpoint_capture.is_some()
                || current.runtime.is_some()
                || current.work.step != SandboxWorkStep::None
                || current.work.lease.is_some()
                || current.credential.pending.is_some()
                || !record
                    .approved_services()?
                    .iter()
                    .all(|service| current.credential.effective_reaches().contains(service))
            {
                return Err(AgentAccessError::Conflict.into());
            }
            let mut command = SandboxLifecycleCommand::revive(
                current.id.clone(),
                record.requester.owner.clone(),
                requested_at.clone(),
                SandboxReviveCause::ExplicitRequest,
            );
            command.managed_runtime_version = store
                .managed_resume_destination(&current, session)
                .await
                .map_err(domain)?;
            match apply_sandbox_lifecycle_command(current.clone(), &command).map_err(domain)? {
                SandboxLifecycleCommandApplication::Commit {
                    expected_revision,
                    sandbox,
                    event,
                } => (expected_revision, sandbox, event),
                _ => return Err(AgentAccessError::Conflict.into()),
            }
        }
    };
    if store
        .legacy_reuse_blocks_quiescence(&current, &sandbox, session)
        .await
        .map_err(driver)?
    {
        return Ok(complete(record, SandboxMaintenanceOutcome::NotEligible));
    }
    validate_in_transaction(store, &current.id, last_seq, session)
        .await
        .map_err(ApplyError::from)?;
    super::super::resource_access::fence_lineage(store, &lineage.resource, session)
        .await
        .map_err(|error| match error {
            super::super::resource_access::ReadError::Driver(error) => driver(error),
            _ => AgentAccessError::Conflict.into(),
        })?;
    fence_live_agents(store, &agents, session).await?;
    let advanced = store.sandboxes.update_one(
        crate::store::work_schema::query(doc! {
            "_id": current.id.as_str(),
            "document_schema_version": DOCUMENT_SCHEMA_VERSION,
            "channel_reuse_cleanup_execution": { "$exists": false },
            "revision": signed(expected_revision, "consented pause revision").map_err(|_| AgentAccessError::Conflict)?,
            "last_seq": signed(last_seq, "consented pause event sequence").map_err(|_| AgentAccessError::Conflict)?,
        }),
        crate::store::work_lease_update::replacement(sandbox_replacement_fields(&sandbox).map_err(domain)?),
    ).session(&mut *session).await.map_err(driver)?;
    if advanced.matched_count != 1 {
        return Err(AgentAccessError::Conflict.into());
    }
    crate::store::sandbox_usage::save::record_saved(store, session, &sandbox, expected_revision)
        .await
        .map_err(ApplyError::from)?;
    let event = append_in_transaction(store, session, &current.id, requested_at.clone(), event)
        .await
        .map_err(ApplyError::from)?;
    let mut predicate = access::lease_filter(lease, DateTime::now().timestamp_millis());
    predicate.insert(
        "record.revision",
        signed(record.revision, "access request revision")
            .map_err(|_| AgentAccessError::Conflict)?,
    );
    record.revision = record
        .revision
        .checked_add(1)
        .filter(|revision| *revision <= i64::MAX as u64)
        .ok_or(AgentAccessError::Unavailable)?;
    record.phase = match action {
        #[cfg(test)]
        Action::Pause => AgentAccessPhase::Pausing,
        Action::Revive => AgentAccessPhase::Reviving,
    };
    record.updated_at = requested_at;
    let operation_id = Some(
        sandbox
            .lifecycle
            .operation
            .as_ref()
            .ok_or(AgentAccessError::Unavailable)?
            .authority()
            .operation_id
            .clone(),
    );
    match action {
        #[cfg(test)]
        Action::Pause => record.progress.pause_operation_id = operation_id,
        Action::Revive => record.progress.revive_operation_id = operation_id,
    }
    let advanced = store
        .agent_access_requests
        .replace_one(predicate, access::document(&record)?)
        .session(&mut *session)
        .await
        .map_err(driver)?;
    if advanced.matched_count != 1 {
        return Err(AgentAccessError::Conflict.into());
    }
    Ok(Receipt {
        maintenance: MaintenanceReceipt {
            sandbox,
            event: Some(event),
            outcome: SandboxMaintenanceOutcome::Scheduled,
            refusal: None,
            unproven_logs: Vec::new(),
        },
        access: record,
    })
}

fn latest_operation_matches(sandbox: &Sandbox, expected: &str) -> bool {
    sandbox
        .lifecycle
        .next_authority
        .checked_sub(1)
        .is_some_and(|generation| {
            expected == format!("sandbox-lifecycle-v1:{}:{generation}", sandbox.id.as_str())
        })
}

async fn reconcile(
    store: &MongoSandboxStore,
    receipt: &Receipt,
    time: Timestamp,
) -> Result<bool, AgentAccessError> {
    if !super::reconcile(store, &receipt.maintenance, time)
        .await
        .map_err(|_| AgentAccessError::Unavailable)?
    {
        return Ok(false);
    }
    let mut session = store
        .client
        .start_session()
        .snapshot(true)
        .snapshot_time(time)
        .await
        .map_err(|_| AgentAccessError::Unavailable)?;
    let actual = store
        .agent_access_requests
        .find_one(doc! { "_id": &receipt.access.id })
        .read_concern(ReadConcern::snapshot())
        .session(&mut session)
        .await
        .map_err(|_| AgentAccessError::Unavailable)?;
    Ok(actual.as_ref().map(access::read).transpose()?.as_ref() == Some(&receipt.access))
}

#[cfg(test)]
#[path = "consented_tests.rs"]
mod tests;

#[cfg(test)]
#[path = "consented_revival_tests.rs"]
mod revival_tests;

#[cfg(test)]
#[path = "consented_scope_tests.rs"]
mod scope_tests;
