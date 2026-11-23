//! Retained complete scan storage, preserving each row and summary owner.
use super::{cursor::ScanInput, *};
use crate::store::ingress_reads::error::ReadFailure;
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmissionRefusal, AdmittedError, AdmittedOperation, AllocationPlan,
    IngressClass, IngressOwnershipPolicy, IngressWorkScope, OwnedWork, SharedOwned,
};
use std::alloc::Layout;

#[cfg_attr(test, derive(Debug))]
pub(in crate::store::agent_log_integrity) struct CapturedHistory {
    pub(in crate::store::agent_log_integrity) entries: CapturedEntries,
    pub(in crate::store::agent_log_integrity) summary: OwnedWork<LogSummary>,
}

#[cfg_attr(test, derive(Debug))]
pub(in crate::store::agent_log_integrity) struct CapturedEntries {
    entries: Vec<SharedOwned<AgentLogEntry>>,
    // Field order keeps the container charged through its physical deallocation.
    storage: IngressWorkScope,
    limit: usize,
}

impl CapturedEntries {
    fn plan(count: usize) -> Result<AllocationPlan, AdmissionRefusal> {
        let bytes = Layout::array::<SharedOwned<AgentLogEntry>>(count)
            .map_err(|_| AdmissionRefusal::InvalidPlan {
                phase: "captured history container",
            })?
            .size();
        AllocationPlan::checked(
            "captured history container",
            &[bytes, shared_owner_bytes::<()>()],
        )
    }

    fn new(count: usize, policy: &IngressOwnershipPolicy) -> Result<Self, AdmittedError> {
        let storage = IngressWorkScope::new(policy.admit(IngressClass::Data, Self::plan(count)?)?);
        Ok(Self {
            entries: Vec::with_capacity(count),
            storage,
            limit: count,
        })
    }

    fn push(
        &mut self,
        entry: SharedOwned<AgentLogEntry>,
    ) -> Result<(), OwnedWork<SandboxStoreError>> {
        if self.entries.len() == self.limit {
            return Err(self
                .storage
                .initialize_owned(|| SandboxStoreError::SequenceConflict));
        }
        self.entries.push(entry);
        Ok(())
    }

    pub(in crate::store::agent_log_integrity) fn as_slice(&self) -> &[SharedOwned<AgentLogEntry>] {
        &self.entries
    }

    fn finish(
        self,
        summary: OwnedWork<LogSummary>,
    ) -> OwnedWork<Result<CapturedHistory, ReadFailure>> {
        self.storage.clone().initialize_owned(|| {
            Ok(CapturedHistory {
                entries: self,
                summary,
            })
        })
    }
}

type CaptureRead<'a> = AdmittedOperation<'a, Result<CapturedHistory, ReadFailure>>;

/// Capture the existing structural scan. Borrowed inputs/session and the source
/// plan retain their existing provenance and driver-qualification obligations.
pub(in crate::store::agent_log_integrity) fn capture_collection_admitted<'a, C: Clock + 'a>(
    caller: ScanCaller,
    input: ScanInput<'a>,
    session: &'a mut mongodb::ClientSession,
    options: ScanOptions<C>,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<CaptureRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked("complete history capture", &[])?,
        move |scope| async move {
            if let Err(error) = rows::require_head(input.last_seq) {
                return Ok(scope.initialize_owned(|| Err(ReadFailure::Domain(error))));
            }
            let count =
                usize::try_from(input.last_seq).map_err(|_| AdmissionRefusal::InvalidPlan {
                    phase: "captured history length",
                })?;
            let mut entries = CapturedEntries::new(count, policy)?;
            let result = cursor::scan_collection_admitted(
                caller,
                input,
                session,
                options,
                policy,
                read_plan,
                |entry| entries.push(entry),
            )?
            .await?;
            match result.transpose() {
                Ok(summary) => Ok(entries.finish(summary)),
                Err(error) => Ok(error.map(Err)),
            }
        },
    )
}

#[cfg(test)]
mod fixtures {
    use super::*;
    use crate::store::ingress_reads::row;
    use layer2_core::sandbox::AgentLogKind;
    use mongodb::{Client, Collection, Database};
    use std::time::{Duration, Instant};
    pub(super) fn plan() -> AllocationPlan {
        AllocationPlan::checked("capture source fixture", &[50 * 1024 * 1024]).unwrap()
    }
    pub(super) fn entries(count: u64, bytes: usize) -> Vec<AgentLogEntry> {
        (1..=count)
            .map(|seq| AgentLogEntry {
                agent_id: AgentId("agt_history_capture".into()),
                seq,
                appended_at: "2026-01-01T00:00:00Z".into(),
                kind: AgentLogKind::AgentMessage {
                    content: "x".repeat(bytes),
                },
            })
            .collect()
    }
    pub(super) fn owned(
        entry: &AgentLogEntry,
        policy: &IngressOwnershipPolicy,
    ) -> SharedOwned<AgentLogEntry> {
        let raw = RawDocumentBuf::try_from(&log_document(entry).unwrap()).unwrap();
        row::decode::<proof::ingress::TailSchema>(&raw, policy).unwrap()
    }
    pub(super) async fn fixture() -> (Client, Database, Collection<Document>) {
        let client = Client::with_uri_str(std::env::var("CHRONO_TEST_MONGODB_URI").unwrap())
            .await
            .unwrap();
        let db = client.database(&format!("cma_capture_{}", uuid::Uuid::new_v4().simple()));
        let collection = db.collection("agent_log");
        (client, db, collection)
    }
    pub(super) async fn insert(collection: &Collection<Document>, values: &[AgentLogEntry]) {
        if !values.is_empty() {
            collection
                .insert_many(values.iter().map(|v| log_document(v).unwrap()))
                .await
                .unwrap();
        }
    }
    #[derive(Clone)]
    pub(super) struct StepClock(Instant);
    impl Clock for StepClock {
        fn now(&self) -> Instant {
            self.0 + Duration::from_secs(1)
        }
    }
    pub(super) fn options(snapshot: bool, batches: bool) -> ScanOptions<StepClock> {
        let mut policy = Policy::enabled(true);
        policy.section_budget = Duration::ZERO;
        ScanOptions::new(snapshot, policy)
            .with_raw_batches(batches)
            .with_clock(StepClock(Instant::now()))
    }
    pub(super) fn capture<'a>(
        collection: &'a Collection<Document>,
        id: &'a AgentId,
        last_seq: u64,
        session: &'a mut mongodb::ClientSession,
        options: ScanOptions<StepClock>,
        policy: &'a IngressOwnershipPolicy,
    ) -> Result<CaptureRead<'a>, AdmittedError> {
        capture_collection_admitted(
            ScanCaller::Other,
            ScanInput {
                collection,
                agent_id: id,
                last_seq,
            },
            session,
            options,
            policy,
            plan(),
        )
    }
    pub(super) fn retained_bytes(
        values: &[AgentLogEntry],
        through: usize,
        policy: &IngressOwnershipPolicy,
    ) -> usize {
        CapturedEntries::plan(values.len())
            .unwrap()
            .requested_bytes()
            + size_of::<LogSummary>()
            + shared_owner_bytes::<()>()
            + values[..through]
                .iter()
                .map(|v| owned(v, policy).requested_bytes())
                .sum::<usize>()
    }
    pub(super) async fn marker(
        collection: &Collection<Document>,
        session: &mut mongodb::ClientSession,
    ) {
        assert!(collection
            .find_one(doc! {"_id":"pending"})
            .session(session)
            .await
            .unwrap()
            .is_some());
        assert!(collection
            .find_one(doc! {"_id":"pending"})
            .await
            .unwrap()
            .is_none());
    }
}

#[cfg(test)]
#[path = "scan_capture_ingress_tests.rs"]
mod tests;
