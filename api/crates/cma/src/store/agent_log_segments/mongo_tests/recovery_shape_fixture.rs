//! Synthetic storage-shape fixtures, not encoded or authoritative recovery work.
//! Actual codec-generated rows require the separately pinned S5 qualification.
use super::*;

pub(super) fn carrier(phase: &str) -> Document {
    assert!(["admitted", "attaching", "completed", "superseded"].contains(&phase));
    let attaching = phase == "attaching";
    let completed = phase == "completed";
    let lease = if attaching {
        bson::Bson::Document(doc! {"holder":"fixture-holder","fence":1_i64,"expires_at":NOW})
    } else {
        bson::Bson::Null
    };
    let completed_at = if completed {
        bson::Bson::String(NOW.into())
    } else {
        bson::Bson::Null
    };
    doc! {
        "_id":format!("capacity-recovery:{}", "1".repeat(64)),
        "format_version":2_i64,
        "agent_id":"agt_segment_fixture",
        "work":{
            "format_version":1_i32,
            "request":{
                "agent_id":"agt_segment_fixture","creation_key":"recovery-fixture",
                "creation_fingerprint":"2".repeat(64),"failure_seq":2_i64,"requested_at":NOW,
            },
            "target":{
                "agent_id":"agt_segment_fixture","creation_key":"recovery-fixture",
                "creation_fingerprint":"2".repeat(64),"conversation_id":"fixture-conversation",
                "conversation_generation":1_i64,
                "runtime":{"runtime_id":"fixture-runtime","runtime_version":"fixture-version","bound_at":NOW},
                "home_identity":"fixture-home",
                "original_open":{"step":"open_conversation","operation_id":"fixture-open",
                    "request_sha256":"3".repeat(64),"completed_at":NOW},
                "active_turn":bson::Bson::Null,"input_sha256":"4".repeat(64),
            },
            "phase":phase,"revision":1_i64,"attempts":0_i64,"next_fence":2_i64,
            "lease":lease,"ready_at":NOW,"completed_at":completed_at,
        },
        "result":{
            "agent_digest":digest::binary(vec![5;32]),"logs_digest":digest::binary(vec![6;32]),
            "agent_revision":1_i64,"through_seq":2_i64,"published_at":NOW,
        },
    }
}
