//! Complete report-only assessment with retained source and visitor custody.
use super::*;
use crate::store::ingress_reads::{
    error::{self, ReadFailure},
    row::{self, ReadError},
};
use layer2_core::sandbox::ingress_ownership::{
    sizing::shared_owner_bytes, AdmittedError, AdmittedOperation, AllocationPlan, IngressClass,
    IngressOwnershipPolicy, OwnedWork, SharedOwned,
};
use read::ingress::{validate_summary_admitted, ProofInput, ReaderFlagsSchema};

pub(in crate::store::agent_log_integrity) struct AssessmentInput<'a> {
    pub(in crate::store::agent_log_integrity) store: &'a MongoSandboxStore,
    pub(in crate::store::agent_log_integrity) agent_id: &'a AgentId,
    pub(in crate::store::agent_log_integrity) last_seq: u64,
}

type AssessmentRead<'a> = AdmittedOperation<'a, Result<LogSummary, ReadFailure>>;

/// Preserve the report-only scanner's optional Agent and caller-supplied head.
/// Input/session provenance, callback captures and the qualified source plan
/// remain caller obligations. This port never aborts or quarantines.
#[cfg_attr(
    not(test),
    expect(
        dead_code,
        reason = "enabled recording awaits complete ingress composition"
    )
)]
pub(in crate::store::agent_log_integrity) fn scan_report_only_admitted<'a, F>(
    caller: ScanCaller,
    input: AssessmentInput<'a>,
    session: &'a mut mongodb::ClientSession,
    snapshot: bool,
    policy: &'a IngressOwnershipPolicy,
    read_plan: AllocationPlan,
    mut visit: F,
) -> Result<AssessmentRead<'a>, AdmittedError>
where
    F: FnMut(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>> + Send + 'a,
{
    AdmittedOperation::from_owned(
        policy,
        IngressClass::Transformation,
        AllocationPlan::checked(
            "report-only assessment result and lineage",
            &[128, shared_owner_bytes::<ReadFailure>()],
        )?,
        move |scope| async move {
            let AssessmentInput {
                store,
                agent_id,
                last_seq,
            } = input;
            let fresh = match store
                .integrity_agent_read_admitted(agent_id, session, snapshot, policy, read_plan)?
                .await?
                .transpose()
            {
                Ok(fresh) => fresh,
                Err(error) => return Ok(error.map(Err)),
            };
            if let Some(fresh) = fresh.as_ref() {
                let flags = match row::decode::<ReaderFlagsSchema<true>>(&fresh.raw, policy) {
                    Ok(flags) => flags,
                    Err(error) => return error::finish(Err(ReadError::Row(error)), &scope),
                };
                if flags.segmented {
                    return Ok(scope.initialize_owned(|| {
                        Err(ReadFailure::Domain(SandboxStoreError::Unavailable(
                            "segmented history requires a bounded reader".into(),
                        )))
                    }));
                }
                if let Err(error) = validate_summary_admitted(
                    store,
                    ProofInput {
                        agent: &fresh.agent,
                        raw: &fresh.raw,
                    },
                    caller,
                    session,
                    snapshot,
                    policy,
                    read_plan,
                )?
                .await?
                .transpose()
                {
                    return Ok(error.map(Err));
                }
            }
            let result = cursor::scan_collection_admitted(
                caller,
                cursor::ScanInput {
                    collection: &store.agent_log,
                    agent_id,
                    last_seq,
                },
                session,
                ScanOptions::new(snapshot, store.scan_cooperation),
                policy,
                read_plan,
                |entry| {
                    if let Some(fresh) = fresh.as_ref() {
                        scope
                            .initialize_owned(|| {
                                read::validate_creation_lineage(&fresh.agent, &entry)
                            })
                            .transpose()?;
                    }
                    visit(entry)
                },
            )?
            .await;
            result
        },
    )
}

#[cfg(test)]
mod fixtures {
    use super::*;
    use crate::store::agent_log_telemetry::snapshot;
    use crate::store::agents::agent_document;
    use crate::store::sandbox_maintenance::mongo_integration_tests::fixture;
    use crate::store::sandbox_mongo::start_transaction;
    use layer2_core::sandbox::AgentLogKind;

    pub(super) fn plan() -> AllocationPlan {
        AllocationPlan::checked("report-only source", &[50 * 1024 * 1024]).unwrap()
    }
    pub(super) fn assess<'a, F>(
        store: &'a MongoSandboxStore,
        agent: &'a Agent,
        session: &'a mut mongodb::ClientSession,
        snapshot: bool,
        policy: &'a IngressOwnershipPolicy,
        visit: F,
    ) -> Result<AssessmentRead<'a>, AdmittedError>
    where
        F: FnMut(SharedOwned<AgentLogEntry>) -> Result<(), OwnedWork<SandboxStoreError>>
            + Send
            + 'a,
    {
        scan_report_only_admitted(
            ScanCaller::Other,
            AssessmentInput {
                store,
                agent_id: &agent.id,
                last_seq: agent.conversation.last_seq,
            },
            session,
            snapshot,
            policy,
            plan(),
            visit,
        )
    }
    pub(super) async fn legacy<F>(
        store: &MongoSandboxStore,
        agent: &Agent,
        session: &mut mongodb::ClientSession,
        snapshot: bool,
        visit: F,
    ) -> Result<LogSummary, TransactionLogError>
    where
        F: FnMut(AgentLogEntry) -> Result<(), SandboxStoreError>,
    {
        super::super::scan_complete_log(
            ScanCaller::Other,
            store,
            &agent.id,
            agent.conversation.last_seq,
            session,
            CompleteScanOptions {
                snapshot,
                failure_policy: IntegrityFailurePolicy::ReportOnly,
            },
            visit,
        )
        .await
    }
    pub(super) async fn seed(
        store: &MongoSandboxStore,
        name: &str,
        count: u64,
    ) -> (Agent, Vec<Document>) {
        let (_, mut agent) = fixture(store, name).await;
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
                            content: "x".repeat(16 * 1024),
                        }
                    },
                })
                .unwrap()
            })
            .collect::<Vec<_>>();
        replace_rows(store, &agent, &rows).await;
        (agent, rows)
    }
    pub(super) async fn replace_rows(store: &MongoSandboxStore, agent: &Agent, rows: &[Document]) {
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
    pub(super) async fn begin(store: &MongoSandboxStore, session: &mut mongodb::ClientSession) {
        start_transaction(session).await.unwrap();
        store
            .agent_log
            .insert_one(doc! {"_id":"report-marker"})
            .session(session)
            .await
            .unwrap();
    }
    pub(super) fn compare(
        expected: Result<LogSummary, TransactionLogError>,
        actual: OwnedWork<Result<LogSummary, ReadFailure>>,
        policy: &IngressOwnershipPolicy,
    ) {
        assert!(actual.belongs_to(policy));
        match (expected, actual.transpose()) {
            (Ok(expected), Ok(actual)) => assert_eq!(expected, *actual),
            (Err(TransactionLogError::Domain(expected)), Err(actual)) => {
                assert!(matches!(&*actual, ReadFailure::Domain(error) if *error == expected))
            }
            other => panic!("report-only outcome changed: {other:?}"),
        }
    }
    pub(super) fn counts() -> [[u64; 3]; 2] {
        [false, true].map(|success| {
            let value = snapshot(ScanCaller::Other, success);
            [value[0], value[1], value[2]]
        })
    }
}

#[cfg(test)]
#[path = "scan_assessment_ingress_tests.rs"]
mod tests;
