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
