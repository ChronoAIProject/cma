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

fn prepare_report_entries(
    last_seq: u64,
    policy: &IngressOwnershipPolicy,
) -> Result<Option<CapturedEntries>, AdmittedError> {
    // Preserve the assessment's original head error after Agent/proof checks.
    if last_seq > PHYSICAL_AGENT_LOG_ENTRIES {
        return Ok(None);
    }
    let count = usize::try_from(last_seq).map_err(|_| AdmissionRefusal::InvalidPlan {
        phase: "report capture length",
    })?;
    CapturedEntries::new(count, policy).map(Some)
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

/// Preserve report-only Agent/proof precedence and the caller's independent head.
/// Capture admission follows preflight and never replaces it with a head error.
pub(in crate::store::agent_log_integrity) fn capture_report_only_admitted<'a>(
    caller: ScanCaller,
    input: assessment::AssessmentInput<'a>,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
) -> Result<CaptureRead<'a>, AdmittedError> {
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked("report-only history capture", &[])?,
        move |scope| async move {
            let mut captured = None;
            let entries = &mut captured;
            let last_seq = input.last_seq;
            let result = assessment::scan_report_only_prepared_admitted(
                caller,
                input,
                session,
                snapshot,
                policy,
                read_plan,
                move || {
                    *entries = prepare_report_entries(last_seq, policy)?;
                    Ok(move |entry| match entries {
                        Some(entries) => entries.push(entry),
                        None => Err(scope.initialize_owned(capacity_error)),
                    })
                },
            )?
            .await?;
            match result.transpose() {
                Ok(summary) => Ok(captured
                    .ok_or(AdmissionRefusal::InvalidPlan {
                        phase: "successful report capture requires a container",
                    })?
                    .finish(summary)),
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
mod report_fixtures {
    use super::*;
    use crate::store::agent_log_integrity::proof::summary_tests::{metadata, policy};
    use crate::store::agent_log_telemetry::snapshot as telemetry;
    use crate::store::agents::agent_document;
    use crate::store::sandbox_maintenance::mongo_integration_tests::fixture;
    use crate::store::sandbox_mongo::start_transaction;
    use layer2_core::sandbox::AgentLogKind;
    pub(super) fn report_row(agent: &Agent, case: &str) -> Document {
        let mut row = agent_document(agent).unwrap();
        let mut proof = metadata(agent, [0; HASH_BYTES]);
        if case == "identity" {
            proof.history_identity = [1; HASH_BYTES];
        }
        row.insert(INTEGRITY_FIELD, proof.to_document().unwrap());
        match case {
            "absent" => {
                row.remove(INTEGRITY_FIELD);
            }
            "quarantine" => {
                row.insert(QUARANTINE_FIELD, false);
            }
            "malformed" => {
                row.insert(INTEGRITY_FIELD, "bad");
            }
            "bad-agent" => {
                row.insert("last_seq", -1_i64);
            }
            "segmented" => {
                row.insert("document_schema_version", 2_i64);
                row.insert("agent_log_format", "segmented_v1");
                row.insert(
                    "agent_log_incarnation",
                    crate::store::agent_log_segments::incarnation(agent),
                );
                row.insert("agent_log_head_revision", 1_i64);
            }
            "unsupported" => {
                let mut deep = doc! {"leaf":true};
                for _ in 0..66 {
                    deep = doc! {"nested":deep};
                }
                row.insert("unused", deep);
            }
            _ => {}
        }
        row
    }
    pub(super) async fn seed(store: &MongoSandboxStore, count: u64) -> (Agent, Vec<Document>) {
        let (_, mut agent) = fixture(store, "sbx_captured_report").await;
        agent.conversation.last_seq = count;
        agent.conversation.conversation_id = None;
        agent.conversation.runtime_generation = None;
        store
            .agents
            .replace_one(
                doc! {"_id":agent.id.as_str()},
                agent_document(&agent).unwrap(),
            )
            .await
            .unwrap();
        let rows = (1..=count)
            .map(|seq| {
                log_document(&AgentLogEntry {
                    agent_id: agent.id.clone(),
                    seq,
                    appended_at: "2026-01-01T00:00:00Z".into(),
                    kind: if seq == 1 {
                        AgentLogKind::Created {
                            workspace_id: agent.workspace_id.clone(),
                            worktree_id: agent.creation_worktree_id().clone(),
                        }
                    } else {
                        AgentLogKind::AgentMessage {
                            content: "x".repeat(256),
                        }
                    },
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        replace(store, &agent, &rows).await;
        (agent, rows)
    }
    pub(super) async fn replace(store: &MongoSandboxStore, agent: &Agent, rows: &[Document]) {
        store
            .agent_log
            .delete_many(doc! {"agent_id":agent.id.as_str()})
            .await
            .unwrap();
        if !rows.is_empty() {
            store
                .agent_log
                .insert_many(rows.iter().cloned())
                .await
                .unwrap();
        }
    }
    pub(super) fn input<'a>(
        store: &'a MongoSandboxStore,
        agent: &'a Agent,
        last_seq: u64,
    ) -> assessment::AssessmentInput<'a> {
        assessment::AssessmentInput {
            store,
            agent_id: &agent.id,
            last_seq,
        }
    }
    pub(super) async fn begin(store: &MongoSandboxStore, session: &mut mongodb::ClientSession) {
        start_transaction(session).await.unwrap();
        store
            .agent_log
            .insert_one(doc! {"_id":"capture-report-marker"})
            .session(session)
            .await
            .unwrap();
    }
    pub(super) fn counts() -> [[u64; 3]; 2] {
        [false, true].map(|success| {
            let v = telemetry(ScanCaller::Other, success);
            [v[0], v[1], v[2]]
        })
    }
    pub(super) fn compare(
        expected: Result<OwnedWork<Result<LogSummary, ReadFailure>>, AdmittedError>,
        actual: Result<OwnedWork<Result<CapturedHistory, ReadFailure>>, AdmittedError>,
    ) {
        match (expected, actual) {
            (Err(expected), Err(actual)) => {
                assert_eq!(format!("{expected:?}"), format!("{actual:?}"))
            }
            (Ok(expected), Ok(actual)) => match (expected.transpose(), actual.transpose()) {
                (Ok(expected), Ok(actual)) => {
                    assert_eq!(*expected, *actual.summary);
                    assert_eq!(
                        actual.entries.as_slice().len() as u64,
                        expected.entry_count()
                    );
                }
                (Err(expected), Err(actual)) => assert!(
                    matches!((&*expected,&*actual),(ReadFailure::Domain(e),ReadFailure::Domain(a)) if e==a)
                ),
                other => panic!("capture changed domain outcome: {other:?}"),
            },
            other => panic!("capture changed admission outcome: {other:?}"),
        }
    }

    #[test]
    pub(super) fn oversized_report_heads_defer_without_container_admission_even_under_data_pressure(
    ) {
        let (root, policy) = policy();
        let idle = root.available_bytes();
        let hold = policy
            .admit(
                IngressClass::Data,
                AllocationPlan::checked(
                    "pressure",
                    &[policy.budget(IngressClass::Data).available_bytes()],
                )
                .unwrap(),
            )
            .unwrap();
        for head in [PHYSICAL_AGENT_LOG_ENTRIES + 1, u64::MAX] {
            assert!(prepare_report_entries(head, &policy).unwrap().is_none());
        }
        assert!(matches!(
            prepare_report_entries(0, &policy),
            Err(AdmittedError::Refused(_))
        ));
        drop(hold);
        assert_eq!(root.available_bytes(), idle);
        let empty = prepare_report_entries(0, &policy).unwrap().unwrap();
        assert!(empty.as_slice().is_empty());
        drop(empty);
        assert_eq!(root.available_bytes(), idle);
    }
}

#[cfg(test)]
#[path = "scan_capture_ingress_tests.rs"]
mod tests;
