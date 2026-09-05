use layer3_adapter::{CheckpointReceipt, ConversationSnapshot, WriterState};
use sha2::{Digest, Sha256};

use super::lifecycle_test_support::{checkpoint, sandbox};
use super::*;
use crate::ids::AgentId;

fn capture(agent_id: &str, conversation_id: &str, bytes: &[u8]) -> SandboxCheckpointConversation {
    SandboxCheckpointConversation::new(
        AgentId(agent_id.into()),
        conversation_id,
        ConversationSnapshot {
            conversation_id: conversation_id.into(),
            archive_sha256: format!("{:x}", Sha256::digest(bytes)),
            archive: bytes.to_vec(),
        },
    )
    .unwrap()
}

fn receipt(epoch: u64) -> CheckpointReceipt {
    CheckpointReceipt {
        epoch,
        object_reference: format!("sandboxes/sbx_lifecycle/checkpoint-{epoch}.tar.zst"),
        sha256: "a".repeat(64),
        writers: WriterState::Resumed,
    }
}

#[test]
fn whole_home_checkpoint_seals_canonical_content_free_metadata() {
    let captures = vec![
        capture("agt_b", "conversation-b", b"private-b"),
        capture("agt_a", "conversation-a", b"private-a"),
    ];
    let receipt = receipt(7);
    let checkpoint =
        SandboxCheckpointRef::seal(&receipt, &captures, "2026-09-05T00:10:00Z").unwrap();

    assert_eq!(checkpoint.epoch, 7);
    assert_eq!(checkpoint.conversations[0].agent_id.as_str(), "agt_a");
    assert_eq!(checkpoint.conversations[1].agent_id.as_str(), "agt_b");
    checkpoint.validate().unwrap();
    checkpoint.validate_capture(&receipt, &captures).unwrap();
    assert!(!serde_json::to_string(&checkpoint)
        .unwrap()
        .contains("private"));
    assert!(!format!("{captures:?}").contains("private-a"));
}

#[test]
fn capture_is_bound_to_the_expected_conversation_and_actual_bytes() {
    let bytes = b"captured conversation".to_vec();
    let valid_digest = format!("{:x}", Sha256::digest(&bytes));
    let mismatched = ConversationSnapshot {
        conversation_id: "conversation-other".into(),
        archive_sha256: valid_digest.clone(),
        archive: bytes.clone(),
    };
    assert!(SandboxCheckpointConversation::new(
        AgentId("agt_a".into()),
        "conversation-a",
        mismatched,
    )
    .is_err());

    let corrupt = ConversationSnapshot {
        conversation_id: "conversation-a".into(),
        archive_sha256: valid_digest,
        archive: b"different bytes".to_vec(),
    };
    assert!(
        SandboxCheckpointConversation::new(AgentId("agt_a".into()), "conversation-a", corrupt,)
            .is_err()
    );
}

#[test]
fn sealing_refuses_incomplete_or_ambiguous_conversation_sets() {
    assert!(SandboxCheckpointRef::seal(&receipt(1), &[], "2026-09-05T00:10:00Z").is_err());

    let duplicate_agent = vec![
        capture("agt_a", "conversation-a", b"one"),
        capture("agt_a", "conversation-b", b"two"),
    ];
    assert!(
        SandboxCheckpointRef::seal(&receipt(1), &duplicate_agent, "2026-09-05T00:10:00Z",).is_err()
    );

    let duplicate_conversation = vec![
        capture("agt_a", "conversation-a", b"one"),
        capture("agt_b", "conversation-a", b"two"),
    ];
    assert!(SandboxCheckpointRef::seal(
        &receipt(1),
        &duplicate_conversation,
        "2026-09-05T00:10:00Z",
    )
    .is_err());
}

#[test]
fn every_piece_of_checkpoint_metadata_is_manifest_bound() {
    let checkpoint = checkpoint(1, "2026-09-05T00:05:00Z");
    let mut mutations = Vec::new();

    let mut changed = checkpoint.clone();
    changed.epoch += 1;
    mutations.push(changed);
    let mut changed = checkpoint.clone();
    changed.object_reference.push_str("-other");
    mutations.push(changed);
    let mut changed = checkpoint.clone();
    changed.sha256 = "f".repeat(64);
    mutations.push(changed);
    let mut changed = checkpoint.clone();
    changed.conversations[0].conversation_id.push_str("-other");
    mutations.push(changed);
    let mut changed = checkpoint.clone();
    changed.sealed_at = "2026-09-05T00:05:01Z".into();
    mutations.push(changed);

    for mutation in mutations {
        assert!(mutation.validate().is_err());
    }
}

#[test]
fn restore_projection_contains_only_the_verified_home_object() {
    let checkpoint = checkpoint(4, "2026-09-05T00:05:00Z");
    let request = checkpoint.restore_request().unwrap();
    assert_eq!(request.object_reference, checkpoint.object_reference);
    assert_eq!(request.sha256, checkpoint.sha256);
}

#[test]
fn history_retains_exactly_latest_and_immediately_previous() {
    let first = checkpoint(1, "2026-09-05T00:05:00Z");
    let second = checkpoint(2, "2026-09-05T00:10:00Z");
    let third = checkpoint(3, "2026-09-05T00:15:00Z");
    let mut history = SandboxCheckpointHistory::default();

    assert_eq!(
        history.publish(first.clone()).unwrap(),
        SandboxCheckpointPublication::Published { evicted: None }
    );
    assert_eq!(
        history.publish(second.clone()).unwrap(),
        SandboxCheckpointPublication::Published { evicted: None }
    );
    assert_eq!(
        history.publish(second.clone()).unwrap(),
        SandboxCheckpointPublication::Replayed
    );
    assert_eq!(
        history.publish(third.clone()).unwrap(),
        SandboxCheckpointPublication::Published {
            evicted: Some(first.clone())
        }
    );
    assert_eq!(history.latest, Some(third));
    assert_eq!(history.previous, Some(second));
    history.validate().unwrap();

    let before = history.clone();
    assert!(history.publish(first).is_err());
    assert_eq!(history, before);
}

#[test]
fn checkpoint_cadence_is_immediate_then_at_least_five_minutes() {
    let mut sandbox = sandbox();
    assert!(sandbox_checkpoint_due(&sandbox, "2026-09-05T00:00:00Z"));

    sandbox.lifecycle.checkpoints.latest = Some(checkpoint(1, "2026-09-05T00:05:00Z"));
    assert!(!sandbox_checkpoint_due(&sandbox, "2026-09-05T00:09:59Z"));
    assert!(sandbox_checkpoint_due(&sandbox, "2026-09-05T00:10:00Z"));
    assert!(!sandbox_checkpoint_due(&sandbox, "not-a-time"));

    sandbox.status = SandboxStatus::Paused;
    assert!(!sandbox_checkpoint_due(&sandbox, "2026-09-05T00:15:00Z"));
}

#[test]
fn sandbox_validation_rejects_a_tampered_checkpoint() {
    let mut sandbox = sandbox();
    sandbox.lifecycle.checkpoints.latest = Some(checkpoint(1, "2026-09-05T00:05:00Z"));
    sandbox.validate().unwrap();
    sandbox
        .lifecycle
        .checkpoints
        .latest
        .as_mut()
        .unwrap()
        .manifest_sha256 = "0".repeat(64);
    assert!(sandbox.validate().is_err());
}
