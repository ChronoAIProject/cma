//! Only explicit jobs enter the cleanup dispatcher. Historical observations
//! remain inert; missing accepted job evidence never permits reconstruction.
use super::*;
use layer2_core::bridge::{cleanup::*, BridgeManagementLease};

mod admission;
mod execution;
mod reads;
mod scheduling;

const JOBS: &str = "channel_child_cleanup_jobs_v1";
const JOB_AUDITS: &str = "channel_child_cleanup_job_admissions_v1";
const JOB_MARKER: &str = "channel_child_cleanup_job_v1";

enum JobError {
    Core(CleanupError),
    Mongo(mongodb::error::Error),
}
impl From<CleanupError> for JobError {
    fn from(error: CleanupError) -> Self {
        Self::Core(error)
    }
}
impl From<mongodb::error::Error> for JobError {
    fn from(error: mongodb::error::Error) -> Self {
        Self::Mongo(error)
    }
}
impl From<AllocationError> for JobError {
    fn from(error: AllocationError) -> Self {
        Self::Core(allocation_error(error))
    }
}
impl From<layer2_core::bridge::BridgeError> for JobError {
    fn from(error: layer2_core::bridge::BridgeError) -> Self {
        Self::Core(error.into())
    }
}
impl From<MutationError> for JobError {
    fn from(error: MutationError) -> Self {
        match error {
            MutationError::Core(error) => error.into(),
            MutationError::Mongo(error) => error.into(),
        }
    }
}
impl JobError {
    fn retryable(&self) -> bool {
        matches!(self, Self::Mongo(error) if is_transient(error) || is_duplicate_key(error))
    }
    fn core(self) -> CleanupError {
        match self {
            Self::Core(error) => error,
            Self::Mongo(_) => CleanupError::Unavailable,
        }
    }
}
fn allocation_error(error: AllocationError) -> CleanupError {
    match error {
        AllocationError::Corrupt => CleanupError::Corrupt,
        AllocationError::Conflict
        | AllocationError::IdentityMismatch
        | AllocationError::InvalidTransition => CleanupError::Conflict,
        _ => CleanupError::Unavailable,
    }
}
fn job_document(job: &CleanupJob) -> Result<Document, CleanupError> {
    job.validate()?;
    Ok(
        doc! {"_id":&job.admission.receipt.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "bridge_id":job.admission.receipt.bridge_id.as_str(),"allocation_id":&job.admission.receipt.allocation_id,
        "complete":job.progress == ChildCleanupProgress::Complete,
        "next_attempt_at_ms":job.next_attempt_at_ms,"lease_expires_at_ms":job.lease.as_ref().map(|lease| lease.expires_at_ms),
        "record":record_bson(job).map_err(|_| CleanupError::Corrupt)?},
    )
}
fn read_job(row: &Document) -> Result<CleanupJob, CleanupError> {
    let job: CleanupJob = read_record(row).map_err(|_| CleanupError::Corrupt)?;
    if job_document(&job)?
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
    {
        return Err(CleanupError::Corrupt);
    }
    Ok(job)
}
fn admission_document(admission: &CleanupJobAdmission) -> Result<Document, CleanupError> {
    admission.validate()?;
    Ok(
        doc! {"_id":&admission.receipt.id,"document_schema_version":DOCUMENT_SCHEMA_VERSION,
        "allocation_id":&admission.receipt.allocation_id,
        "record":record_bson(admission).map_err(|_| CleanupError::Corrupt)?},
    )
}
fn read_admission(row: &Document) -> Result<CleanupJobAdmission, CleanupError> {
    let admission: CleanupJobAdmission = read_record(row).map_err(|_| CleanupError::Corrupt)?;
    if admission_document(&admission)?
        .iter()
        .any(|(key, value)| row.get(key) != Some(value))
    {
        return Err(CleanupError::Corrupt);
    }
    Ok(admission)
}
fn holds(holds: &[ChannelReuseCleanupHold]) -> Vec<ChildCleanupHold> {
    holds
        .iter()
        .map(|hold| match hold {
            ChannelReuseCleanupHold::AgentBusy => ChildCleanupHold::AgentBusy,
            ChannelReuseCleanupHold::WorkspaceBusy => ChildCleanupHold::WorkspaceBusy,
            ChannelReuseCleanupHold::PendingCommand => ChildCleanupHold::PendingCommand,
            ChannelReuseCleanupHold::UncertainCommand => ChildCleanupHold::UncertainCommand,
            ChannelReuseCleanupHold::PendingReply => ChildCleanupHold::PendingReply,
            ChannelReuseCleanupHold::UncertainReply => ChildCleanupHold::UncertainReply,
            ChannelReuseCleanupHold::OtherWorkspaceChildren => {
                ChildCleanupHold::OtherWorkspaceChildren
            }
            ChannelReuseCleanupHold::InventoryLimit => ChildCleanupHold::InventoryLimit,
        })
        .collect()
}

impl MongoBridgeAllocationStore {
    async fn cleanup_job_manager(
        &self,
        manager: &BridgeManagementLease,
        session: &mut ClientSession,
    ) -> Result<(), JobError> {
        manager.require_current()?;
        let row = self
            .bridges
            .definitions
            .find_one(doc! {"_id":manager.record().id.as_str()})
            .session(&mut *session)
            .await?
            .ok_or(CleanupError::NotFound)?;
        if crate::store::bridge_definitions::decode_bridge(&row)
            .map_err(|_| CleanupError::Corrupt)?
            != *manager.record()
        {
            return Err(CleanupError::Conflict.into());
        }
        let changed = self
            .bridges
            .definitions
            .update_one(row, doc! {"$inc":{"child_cleanup_read_fence":1_i64}})
            .session(session)
            .await?;
        if changed.matched_count != 1 {
            return Err(CleanupError::Conflict.into());
        }
        manager.require_current()?;
        Ok(())
    }
    async fn cleanup_job_access(
        &self,
        access: &CleanupAccess,
        session: &mut ClientSession,
    ) -> Result<Document, JobError> {
        access.require_current()?;
        self.cleanup_job_manager(access.management(), session)
            .await?;
        let resources = &self
            .invokes
            .as_ref()
            .ok_or(CleanupError::Unavailable)?
            .resources;
        let target = layer2_core::resource_access::ResourceTarget::Sandbox {
            sandbox_id: access.allocation().placement.target().sandbox_id.clone(),
        };
        use crate::store::resource_access::{fence_lineage, read_execution_snapshot, ReadError};
        let convert = |error: ReadError| match error {
            ReadError::Driver(error) => JobError::Mongo(error),
            ReadError::Domain(_) => JobError::Core(CleanupError::Conflict),
        };
        let current = read_execution_snapshot(resources, &target, session)
            .await
            .map_err(convert)?;
        if &current != access.parent() {
            return Err(CleanupError::Conflict.into());
        }
        fence_lineage(resources, &current, session)
            .await
            .map_err(convert)?;
        let row = self
            .records
            .find_one(doc! {"_id":&access.allocation().id})
            .session(&mut *session)
            .await?
            .ok_or(CleanupError::NotFound)?;
        if records::decode_record(&row)? != *access.allocation() {
            return Err(CleanupError::Conflict.into());
        }
        access.require_current()?;
        Ok(row)
    }
}
