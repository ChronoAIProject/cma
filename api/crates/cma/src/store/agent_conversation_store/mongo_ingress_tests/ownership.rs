//! A frozen predecessor reader keeps runtime custody out of persisted BSON.
use layer2_core::sandbox::store::AgentMessageCheckpoint;
use mongodb::bson::{doc, Bson, Document};
use serde::{Deserialize, Serialize};

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorCheckpoint {
    version: u32,
    update_id: String,
    update_fingerprint: String,
    native_conversation_id: String,
    runtime_generation: u64,
    active_turn: Option<u64>,
    after_seq: u64,
    assembly: PredecessorAssembly,
    outcome: PredecessorOutcome,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    output: Option<PredecessorSource>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorAssembly {
    message_id: Option<String>,
    content: String,
    prompt_evidence: bool,
    live_message: Option<PredecessorLive>,
    live_thought: Option<PredecessorLive>,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorLive {
    generation: u64,
    turn: u64,
    source: Option<String>,
    id: String,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorOutcome {
    appended: Vec<PredecessorEntry>,
    turn_closed: bool,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorEntry {
    agent_id: String,
    seq: u64,
    appended_at: String,
    kind: PredecessorKind,
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PredecessorKind {
    AgentMessage { content: String },
}

#[derive(Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorSource {
    stream_id: String,
    cursor: u64,
    native_conversation_id: String,
    payload_sha256: String,
}

fn document() -> Document {
    doc! {
        "version": 1_i64,
        "update_id": "00000000-0000-4000-8000-000000000001",
        "update_fingerprint": "a".repeat(64),
        "native_conversation_id": "synthetic-conversation",
        "runtime_generation": 7_i64,
        "active_turn": 3_i64,
        "after_seq": 9_i64,
        "assembly": {
            "message_id": "synthetic-message", "content": "retained prefix",
            "prompt_evidence": true,
            "live_message": {"generation": 7_i64, "turn": 3_i64, "source": "synthetic-message", "id": "synthetic-live"},
            "live_thought": Bson::Null,
        },
        "outcome": {
            "appended": [{
                "agent_id": "agt_synthetic", "seq": 9_i64,
                "appended_at": "2026-01-01T00:00:00Z",
                "kind": {"type": "agent_message", "content": "published message"},
            }],
            "turn_closed": false,
        },
        "output": {
            "stream_id": "00000000-0000-4000-8000-000000000002",
            "cursor": 19_i64, "native_conversation_id": "synthetic-conversation",
            "payload_sha256": "b".repeat(64),
        },
    }
}

#[test]
fn checkpoint_bson_is_byte_identical_through_the_strict_predecessor() {
    for with_source in [false, true] {
        let mut expected = document();
        if !with_source {
            expected.remove("output");
        }
        let current: AgentMessageCheckpoint =
            mongodb::bson::deserialize_from_document(expected.clone()).unwrap();
        current.validate().unwrap();
        let encoded = mongodb::bson::serialize_to_vec(&current).unwrap();
        assert_eq!(encoded, expected.to_vec().unwrap());
        let previous: PredecessorCheckpoint =
            mongodb::bson::deserialize_from_slice(&encoded).unwrap();
        let predecessor_bytes = mongodb::bson::serialize_to_vec(&previous).unwrap();
        assert_eq!(predecessor_bytes, encoded);
        let restored: AgentMessageCheckpoint =
            mongodb::bson::deserialize_from_slice(&predecessor_bytes).unwrap();
        assert_eq!(restored, current);
    }
}

#[test]
fn predecessor_bson_rejects_runtime_state_at_each_owned_boundary() {
    for field in ["", "assembly", "outcome", "output"] {
        let mut row = document();
        let target = if field.is_empty() {
            &mut row
        } else {
            row.get_document_mut(field).unwrap()
        };
        target.insert("reservation", doc! {"bytes": 4096});
        assert!(mongodb::bson::deserialize_from_document::<PredecessorCheckpoint>(row).is_err());
    }
}

use super::{
    fixture::{projection_fixture, Fixture},
    gate_plan, policy,
};
use crate::store::agent_log_segments::projection::{self, read_admitted as read};
use crate::store::ingress_reads::error::ReadFailure;
use layer2_core::sandbox::{
    ingress_ownership::{AdmittedError, AllocationPlan, IngressClass},
    store::SandboxStoreError,
};

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projection_read_keeps_transaction_replacement_and_independent_retention() {
    let fixture = Fixture::new("projection_retention").await;
    let store = &fixture.store;
    let rows = &store.segment_logs.projections;
    let (agent, head, small) = projection_fixture(&fixture.agent, 1);
    let (grown_agent, _, grown) = projection_fixture(&fixture.agent, 64);
    rows.insert_one(mongodb::bson::serialize_to_document(&small).unwrap())
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let legacy = projection::read(store, &agent, &head, &mut session)
        .await
        .unwrap();
    let result = read(store, &agent, &head, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(result.record, legacy);
    let retained = (*result).clone();
    drop(result);
    let small_available = policy.budget(IngressClass::Data).available_bytes();
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    rows.replace_one(
        doc! {"_id":&head.agent_id},
        mongodb::bson::serialize_to_document(&grown).unwrap(),
    )
    .session(&mut session)
    .await
    .unwrap();
    let changed = read(
        store,
        &grown_agent,
        &head,
        &mut session,
        &policy,
        gate_plan(),
    )
    .unwrap()
    .await
    .unwrap()
    .transpose()
    .unwrap();
    assert_eq!(changed.record, grown.record);
    assert_eq!(retained.record, small.record);
    assert!(retained.belongs_to(&policy));
    assert!(policy.budget(IngressClass::Data).available_bytes() < small_available);
    drop(changed);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        small_available
    );
    session.abort_transaction().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let restored = read(store, &agent, &head, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(restored.record, small.record);
    drop((restored, retained));
    assert_eq!(root.available_bytes(), idle);
    session.abort_transaction().await.unwrap();
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projection_read_preserves_validation_failures_and_outer_preflight_refusal() {
    let fixture = Fixture::new("projection_errors").await;
    let store = &fixture.store;
    let rows = &store.segment_logs.projections;
    let (agent, head, valid) = projection_fixture(&fixture.agent, 1);
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    for fault in [
        "missing",
        "incarnation",
        "generation",
        "through",
        "revision",
        "aggregate",
        "nullable",
        "unknown",
        "deep",
    ] {
        rows.delete_many(doc! {}).await.unwrap();
        let mut row = mongodb::bson::serialize_to_document(&valid).unwrap();
        match fault {
            "incarnation" => {
                row.insert("incarnation", "1".repeat(64));
            }
            "generation" => {
                row.insert("evidence_generation", "replaced");
            }
            "through" => {
                row.insert("through_seq", 127_i64);
            }
            "revision" => {
                row.insert("head_revision", 2_i64);
            }
            "aggregate" => {
                row.get_document_mut("record")
                    .unwrap()
                    .insert("latest_task_status", 0_i64);
            }
            "nullable" => {
                row.get_document_mut("record")
                    .unwrap()
                    .remove("latest_turn");
            }
            "unknown" => {
                row.insert("unexpected", "x".repeat(64 * 1024));
            }
            "deep" => {
                let mut nested = doc! {"leaf":true};
                for _ in 0..66 {
                    nested = doc! {"nested":nested};
                }
                row.insert("unexpected", nested);
            }
            _ => {}
        }
        if fault != "missing" {
            rows.insert_one(row.clone()).await.unwrap();
        }
        assert!(projection::read(store, &agent, &head, &mut session)
            .await
            .is_err());
        let result = read(store, &agent, &head, &mut session, &policy, gate_plan())
            .unwrap()
            .await;
        if fault == "deep" {
            assert!(matches!(result, Err(AdmittedError::Unsupported(_))));
        } else {
            let error = result.unwrap().transpose().unwrap_err();
            assert!(
                matches!(&*error, ReadFailure::Domain(SandboxStoreError::Corrupt(_))),
                "{fault}"
            );
            assert!(error.belongs_to(&policy));
            assert!(root.available_bytes() < idle);
            drop(error);
        }
        assert_eq!(root.available_bytes(), idle);
        if fault != "missing" {
            assert_eq!(
                rows.find_one(doc! {"_id": &head.agent_id}).await.unwrap(),
                Some(row)
            );
        }
    }
    drop(session);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn projection_read_pins_snapshot_and_sizes_each_actual_response_under_pressure() {
    let fixture = Fixture::new("projection_snapshot").await;
    let store = &fixture.store;
    let rows = &store.segment_logs.projections;
    let (agent, head, small) = projection_fixture(&fixture.agent, 1);
    let (grown_agent, _, grown) = projection_fixture(&fixture.agent, 64);
    rows.insert_one(mongodb::bson::serialize_to_document(&small).unwrap())
        .await
        .unwrap();
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().snapshot(true).await.unwrap();
    // Establish the same server snapshot the caller's preceding head read owns.
    rows.find_one(doc! {"_id":&head.agent_id})
        .read_concern(mongodb::options::ReadConcern::snapshot())
        .session(&mut session)
        .await
        .unwrap();
    let timestamp = session.snapshot_time();
    let result = read(store, &agent, &head, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    let charge = policy.bounds().data_bytes() - policy.budget(IngressClass::Data).available_bytes();
    drop(result);
    let pressure = policy
        .admit(
            IngressClass::Data,
            AllocationPlan::checked(
                "projection small allowance",
                &[policy.bounds().data_bytes() - charge],
            )
            .unwrap(),
        )
        .unwrap();
    let held = root.available_bytes();
    rows.replace_one(
        doc! {"_id":&head.agent_id},
        mongodb::bson::serialize_to_document(&grown).unwrap(),
    )
    .await
    .unwrap();
    let pinned = read(store, &agent, &head, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(pinned.record, small.record);
    assert_eq!(session.snapshot_time(), timestamp);
    drop(pinned);
    let mut later = store.client.start_session().await.unwrap();
    assert!(matches!(
        read(store, &grown_agent, &head, &mut later, &policy, gate_plan())
            .unwrap()
            .await,
        Err(AdmittedError::Refused(_))
    ));
    assert_eq!(root.available_bytes(), held);
    drop(pressure);
    let changed = read(store, &grown_agent, &head, &mut later, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(changed.record, grown.record);
    drop((changed, session, later));
    assert_eq!(root.available_bytes(), idle);
    fixture.finish().await;
}

#[tokio::test]
#[ignore = "requires CHRONO_TEST_MONGODB_URI pointing at a disposable replica set"]
async fn parent_read_retains_exact_transaction_rows_and_restores_after_rollback() {
    use crate::store::agent_conversation_store::reopen::read_parent_admitted as read;
    let fixture = Fixture::new("parent_retention").await;
    let store = &fixture.store;
    let rows = &store.sandboxes;
    let mut grown = fixture.parent.clone();
    grown.profile.content.software_install_commands = vec!["synthetic".repeat(8192); 32];
    let (root, policy) = policy();
    let idle = root.available_bytes();
    let mut session = store.client.start_session().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let result = read(store, &fixture.agent, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(**result, fixture.parent);
    let retained = (*result).clone();
    drop(result);
    let small_available = policy.budget(IngressClass::Data).available_bytes();
    assert_eq!(
        policy
            .budget(IngressClass::Transformation)
            .available_bytes(),
        policy.bounds().transformation_bytes()
    );
    rows.update_one(
        doc! {"_id":fixture.parent.id.as_str()},
        doc! {"$set":crate::store::sandboxes::sandbox_replacement_fields(&grown).unwrap()},
    )
    .session(&mut session)
    .await
    .unwrap();
    let changed = read(store, &fixture.agent, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(**changed, grown);
    assert_eq!(*retained, fixture.parent);
    assert!(retained.belongs_to(&policy));
    assert!(policy.budget(IngressClass::Data).available_bytes() < small_available);
    drop(changed);
    assert_eq!(
        policy.budget(IngressClass::Data).available_bytes(),
        small_available
    );
    session.abort_transaction().await.unwrap();
    crate::store::sandbox_mongo::start_transaction(&mut session)
        .await
        .unwrap();
    let restored = read(store, &fixture.agent, &mut session, &policy, gate_plan())
        .unwrap()
        .await
        .unwrap()
        .transpose()
        .unwrap();
    assert_eq!(**restored, fixture.parent);
    drop((restored, retained));
    assert_eq!(root.available_bytes(), idle);
    session.abort_transaction().await.unwrap();
    drop(session);
    fixture.finish().await;
}
