use serde::{Deserialize, Serialize};

use super::{coalesce::AgentOutputCoalescePolicy, output};
use crate::sandbox::conversation::PendingAgentUpdate;
use crate::sandbox::store::AgentMessageCheckpoint;
use crate::sandbox::AgentMessageAssembly;
use layer3_adapter::{ConversationUpdate, RuntimeOutputSource};

// Frozen independently of production DTOs: adding a runtime ownership field to
// a persisted type must fail even if both current readers and writers change.
#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
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

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorAssembly {
    message_id: Option<String>,
    content: String,
    prompt_evidence: bool,
    live_message: Option<PredecessorLiveIdentity>,
    live_thought: Option<PredecessorLiveIdentity>,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorLiveIdentity {
    generation: u64,
    turn: u64,
    source: Option<String>,
    id: String,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorOutcome {
    appended: Vec<PredecessorLog>,
    turn_closed: bool,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorLog {
    agent_id: String,
    seq: u64,
    appended_at: String,
    kind: PredecessorLogKind,
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
enum PredecessorLogKind {
    AgentMessage { content: String },
}

#[derive(Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct PredecessorSource {
    stream_id: String,
    cursor: u64,
    native_conversation_id: String,
    payload_sha256: String,
}

const CHECKPOINT: &str = concat!(
    r#"{"version":1,"update_id":"00000000-0000-4000-8000-000000000001","update_fingerprint":"aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa","native_conversation_id":"synthetic-conversation","runtime_generation":7,"active_turn":3,"after_seq":9,"#,
    r#""assembly":{"message_id":"message-3","content":"retained prefix","prompt_evidence":true,"live_message":{"generation":7,"turn":3,"source":"message-3","id":"live-3"},"live_thought":null},"#,
    r#""outcome":{"appended":[{"agent_id":"00000000-0000-4000-8000-000000000003","seq":9,"appended_at":"2026-01-01T00:00:00Z","kind":{"type":"agent_message","content":"published message"}}],"turn_closed":false},"#,
    r#""output":{"stream_id":"00000000-0000-4000-8000-000000000002","cursor":12,"native_conversation_id":"synthetic-conversation","payload_sha256":"bbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbbb"}}"#,
);

fn checkpoint() -> AgentMessageCheckpoint {
    serde_json::from_str(CHECKPOINT).unwrap()
}

#[test]
fn checkpoint_json_round_trips_through_a_strict_predecessor_reader() {
    let current = checkpoint();
    current.validate().unwrap();
    let encoded = serde_json::to_string(&current).unwrap();
    assert_eq!(encoded, CHECKPOINT);
    let predecessor: PredecessorCheckpoint = serde_json::from_str(&encoded).unwrap();
    let restored: AgentMessageCheckpoint =
        serde_json::from_str(&serde_json::to_string(&predecessor).unwrap()).unwrap();
    assert_eq!(restored, current);
    assert_eq!(restored.version, 1);
}

#[test]
fn checkpoint_without_a_source_preserves_the_legacy_omission() {
    let mut current = checkpoint();
    current.output = None;
    let encoded = serde_json::to_value(&current).unwrap();
    assert!(encoded.get("output").is_none());
    let predecessor: PredecessorCheckpoint = serde_json::from_value(encoded).unwrap();
    assert!(predecessor.output.is_none());
    let restored: AgentMessageCheckpoint =
        serde_json::from_value(serde_json::to_value(predecessor).unwrap()).unwrap();
    assert_eq!(restored, current);
}

#[test]
fn uncertain_and_quiet_runtime_state_never_enters_the_checkpoint() {
    let mut current = checkpoint();
    let policy = AgentOutputCoalescePolicy::new(std::time::Duration::from_secs(1), 128).unwrap();
    let source = current.output.as_ref().unwrap();
    let mut assembly = current.assembly.clone();
    assert!(assembly.retain_quiet(&AgentMessageAssembly::default(), source, 7, policy));
    assembly.pending_update = Some(Box::new(PendingAgentUpdate {
        id: current.update_id.clone(),
        fingerprint: current.update_fingerprint.clone(),
        runtime_generation: current.runtime_generation,
        active_turn: current.active_turn,
        after_seq: current.after_seq,
    }));
    assert_eq!(assembly.buffered_output(), Some(source));
    assert!(assembly.pending_update.is_some());
    assert_eq!(
        serde_json::to_value(&assembly).unwrap(),
        serde_json::to_value(&current.assembly).unwrap(),
    );
    current.assembly = assembly.durable_copy();
    current.validate().unwrap();
    assert!(current.assembly.buffered_output().is_none());
    assert!(current.assembly.pending_update.is_none());
    assert_eq!(serde_json::to_string(&current).unwrap(), CHECKPOINT);
}

#[test]
fn strict_predecessor_refuses_runtime_fields_at_each_ownership_boundary() {
    for pointer in [
        "",
        "/assembly",
        "/assembly/live_message",
        "/outcome",
        "/output",
    ] {
        let mut value: serde_json::Value = serde_json::from_str(CHECKPOINT).unwrap();
        value
            .pointer_mut(pointer)
            .unwrap()
            .as_object_mut()
            .unwrap()
            .insert("reservation".into(), serde_json::json!({"bytes": 512}));
        assert!(serde_json::from_value::<PredecessorCheckpoint>(value).is_err());
    }
}

#[test]
fn source_json_keeps_the_exact_original_retry_identity() {
    let source = checkpoint().output.unwrap();
    let encoded = serde_json::to_string(&source).unwrap();
    let predecessor: PredecessorSource = serde_json::from_str(&encoded).unwrap();
    let restored: RuntimeOutputSource =
        serde_json::from_str(&serde_json::to_string(&predecessor).unwrap()).unwrap();
    assert_eq!(restored, source);
    assert_eq!(
        output::fingerprint(Some(&source), None),
        output::fingerprint(Some(&restored), None)
    );
}

#[test]
fn fingerprint_matches_the_predecessor_debug_encoding() {
    use sha2::{Digest, Sha256};

    let source = checkpoint().output.unwrap();
    assert_eq!(
        output::fingerprint(Some(&source), None),
        format!("{:x}", Sha256::digest(format!("{source:?}").as_bytes())),
    );
    let update = ConversationUpdate::Failed {
        message: "synthetic text with escapes: \n\t\" and Unicode: λ".into(),
        capability: None,
    };
    assert_eq!(
        output::fingerprint(None, Some(&update)),
        format!("{:x}", Sha256::digest(format!("{update:?}").as_bytes())),
    );
}

#[test]
fn fingerprint_streams_maximal_escaped_input_without_changing_identity() {
    use sha2::{Digest, Sha256};

    let update = ConversationUpdate::Failed {
        message: "\n".repeat(4 * 1024 * 1024),
        capability: Some("synthetic-capability".into()),
    };
    let previous = format!("{:x}", Sha256::digest(format!("{update:?}").as_bytes()));
    assert_eq!(output::fingerprint(None, Some(&update)), previous);
}

#[test]
fn durable_copy_preserves_content_capacity_independently_of_quiet_sources() {
    let current = checkpoint();
    let mut assembly = current.assembly;
    assembly.content = "x".repeat(crate::sandbox::MAX_DURABLE_AGENT_MESSAGE_BYTES - 1);
    let policy = AgentOutputCoalescePolicy::new(std::time::Duration::from_secs(1), 128).unwrap();
    let mut source = current.output.unwrap();
    source.native_conversation_id = "n".repeat(4096);
    for cursor in 1..128 {
        source.cursor = cursor;
        let mut next = assembly.clone();
        assert!(next.retain_quiet(&assembly, &source, 7, policy));
        assembly = next;
    }
    let durable = assembly.durable_copy();
    assert_eq!(durable.content, assembly.content);
    assert_ne!(durable.content.as_ptr(), assembly.content.as_ptr());
    assert_eq!(durable.message_id, assembly.message_id);
    assert_eq!(durable.live_message, assembly.live_message);
    assert_eq!(durable.live_thought, assembly.live_thought);
    assert!(durable.output_state.is_none());
    assert_eq!(assembly.buffered_output(), Some(&source));
}
