use super::*;
use crate::store::agent_capacity_recovery::records::test_support::{agent, stored, NOW};
use layer2_core::sandbox::{AgentLogEntry, AgentLogKind, AgentStatus};

fn receipt(agent: &Agent, stored: &StoredRecovery) -> Publication {
    let identity = stored.work.admission_identity().unwrap();
    let key = if stored.work.revision == 1 {
        identity.key
    } else {
        format!("{}:step:{}", identity.key, stored.work.revision)
    };
    let receipt = Publication {
        id: publication::identity(agent.id.as_str(), &incarnation(agent), &key),
        format_version: 1,
        agent_digest: None,
        agent_id: agent.id.0.clone(),
        incarnation: incarnation(agent),
        publication_key: key,
        fingerprint: Binary {
            subtype: BinarySubtype::Generic,
            bytes: identity.fingerprint.to_vec(),
        },
        appended_at: stored.result.published_at.clone(),
        expected_revision: stored.result.agent_revision as i64 - 1,
        expected_seq: stored.result.through_seq as i64
            - i64::from(stored.work.phase == RecoveryPhase::Completed),
        first_seq: stored.result.through_seq as i64
            + i64::from(stored.work.phase != RecoveryPhase::Completed),
        last_seq: stored.result.through_seq as i64,
        result_revision: stored.result.agent_revision as i64,
        head_revision: 1,
        active_ordinal: 0,
        previous_seal: None,
        result_digest: stored.result.digest(&stored.work).unwrap(),
    };
    receipt.validate().unwrap();
    receipt
}

fn completed() -> (
    Agent,
    StoredRecovery,
    Publication,
    Publication,
    AgentLogEntry,
) {
    let admitted = stored();
    let original = agent();
    let admission = receipt(&original, &admitted);
    let mut agent = original;
    agent.revision += 2;
    agent.conversation.last_seq += 1;
    agent.status = AgentStatus::Ready;
    let mut work = admitted.work;
    work.revision = 3;
    work.attempts = 1;
    work.next_fence += 1;
    work.phase = RecoveryPhase::Completed;
    work.completed_at = Some(NOW.into());
    let log = AgentLogEntry {
        agent_id: agent.id.clone(),
        seq: agent.conversation.last_seq,
        appended_at: NOW.into(),
        kind: AgentLogKind::StatusChanged {
            from: AgentStatus::Failed,
            to: AgentStatus::Ready,
        },
    };
    let stored = StoredRecovery::new(&agent, work, std::slice::from_ref(&log), NOW).unwrap();
    let latest = receipt(&agent, &stored);
    (agent, stored, admission, latest, log)
}

#[test]
fn capacity_recovery_receipt_survives_later_head_and_stop_without_replacing_result() {
    let (mut current, stored, admitted, latest, log) = completed();
    verify_binding(&current, &stored, &admitted, &latest).unwrap();
    let original = stored.clone();
    current.revision += 2;
    current.conversation.last_seq += 2;
    current.status = AgentStatus::Stopped;
    current.stopped_at = Some(NOW.into());
    verify_binding(&current, &stored, &admitted, &latest).unwrap();
    stored.result.require_logs(&stored.work, &[log]).unwrap();
    assert_eq!(stored, original);
    // A later head is not the original result; the witness retains the original hash.
    assert_ne!(
        stored.result.agent_digest,
        publication::result_digest(&current).unwrap()
    );
}

#[test]
fn capacity_recovery_original_receipt_survives_output_at_the_same_agent_revision() {
    let (mut current, stored, admitted, latest, _) = completed();
    let original = stored.clone();
    assert!(same_result_position(&current, &stored));
    current.conversation.last_seq += 1;
    assert_eq!(current.revision, stored.result.agent_revision);
    assert!(!same_result_position(&current, &stored));
    verify_binding(&current, &stored, &admitted, &latest).unwrap();
    assert_eq!(stored, original);
    // This only preserves the original receipt. Activation still requires the
    // separate scoped current-publication proof and fresh authority.
    assert_ne!(
        stored.result.agent_digest,
        publication::result_digest(&current).unwrap()
    );
}

#[test]
fn capacity_recovery_receipt_rejects_foreign_incarnation_and_changed_current_result() {
    let (current, stored, admitted, latest, _) = completed();
    for variant in 0..5 {
        let mut changed = current.clone();
        match variant {
            0 => changed.creation_fingerprint = "e".repeat(64),
            1 => changed.id = layer2_core::AgentId("another".into()),
            2 => changed.revision -= 1,
            3 => changed.conversation.last_seq -= 1,
            _ => changed.status = AgentStatus::Stopped,
        }
        assert!(verify_binding(&changed, &stored, &admitted, &latest).is_err());
    }
}

#[test]
fn capacity_recovery_receipt_rejects_carrier_and_publication_tampering() {
    let (current, stored, admitted, latest, _) = completed();
    for variant in 0..7 {
        let mut changed = stored.clone();
        match variant {
            0 => changed.work.attempts += 1,
            1 => changed.work.next_fence += 1,
            2 => changed.work.target.input_sha256 = "e".repeat(64),
            3 => changed.result.agent_digest.bytes[0] ^= 1,
            4 => changed.result.logs_digest.bytes[0] ^= 1,
            5 => changed.result.agent_revision += 1,
            _ => changed.work.request.requested_at = "2026-11-11T00:01:00Z".into(),
        }
        assert!(verify_binding(&current, &changed, &admitted, &latest).is_err());
    }
    for variant in 0..5 {
        let mut changed = latest.clone();
        match variant {
            0 => changed.result_digest.bytes[0] ^= 1,
            1 => changed.publication_key.push_str(":other-attempt"),
            2 => changed.expected_seq -= 1,
            3 => changed.expected_revision -= 1,
            _ => changed.appended_at = "2026-11-11T00:01:00Z".into(),
        }
        assert!(verify_binding(&current, &stored, &admitted, &changed).is_err());
    }
}

#[test]
fn capacity_recovery_completion_requires_exact_retained_status_event() {
    let (_, stored, _, _, log) = completed();
    assert!(stored.result.require_logs(&stored.work, &[]).is_err());
    for variant in 0..4 {
        let mut changed = log.clone();
        match variant {
            0 => changed.seq -= 1,
            1 => changed.agent_id = layer2_core::AgentId("another".into()),
            2 => {
                changed.kind = AgentLogKind::StatusChanged {
                    from: AgentStatus::Failed,
                    to: AgentStatus::Working,
                }
            }
            _ => changed.appended_at = "2026-11-11T00:01:00Z".into(),
        }
        assert!(stored
            .result
            .require_logs(&stored.work, &[changed])
            .is_err());
    }
}
