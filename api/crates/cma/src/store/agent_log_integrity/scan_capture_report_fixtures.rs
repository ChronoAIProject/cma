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
pub(super) fn oversized_report_heads_defer_without_container_admission_even_under_data_pressure() {
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
