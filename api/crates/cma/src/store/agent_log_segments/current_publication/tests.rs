use super::*;
use crate::store::agent_log_segments::{digest, writer_projection::tests::opened};

fn fixture() -> (Agent, Head, Publication) {
    let (current, mut head, _) = opened();
    let mut next = current.clone();
    next.revision += 1;
    head.revision += 1;
    let receipt = Publication::new(
        &current,
        &next,
        &head,
        &publication::PublicationIdentity {
            key: "current-proof-fixture".into(),
            fingerprint: publication::request_digest(&"original-request").unwrap(),
            appended_at: "2026-11-11T00:00:00Z".into(),
        },
        publication::result_digest(&("domain-result", 7)).unwrap(),
    )
    .unwrap();
    (next, head, receipt)
}

#[test]
fn current_publication_accepts_exact_full_agent_independently_of_domain_result() {
    let (agent, head, receipt) = fixture();
    assert_ne!(receipt.agent_digest.as_ref(), Some(&receipt.result_digest));
    require_binding(&agent, &head, &receipt).unwrap();
    assert_eq!(
        Publication::read(bson::serialize_to_document(&receipt).unwrap()).unwrap(),
        receipt
    );
}

#[test]
fn current_publication_refuses_mutable_agent_changes_even_without_revision_growth() {
    let (agent, head, receipt) = fixture();
    for change in [
        |a: &mut Agent| a.revision += 1,
        |a: &mut Agent| a.owner = layer2_core::SubjectId::new("other-fixture"),
        |a: &mut Agent| a.sandbox_id = layer2_core::SandboxId("other-sandbox".into()),
        |a: &mut Agent| a.workspace_id = layer2_core::WorkspaceId("other-workspace".into()),
        |a: &mut Agent| a.worktree_id = layer2_core::WorktreeId("other-worktree".into()),
        |a: &mut Agent| a.status = layer2_core::sandbox::AgentStatus::Failed,
        |a: &mut Agent| a.conversation.runtime_generation = Some(2),
        |a: &mut Agent| a.conversation.conversation_id = Some("other-conversation".into()),
        |a: &mut Agent| a.work.next_fence += 1,
        |a: &mut Agent| {
            a.requested_stop_reason = Some(layer2_core::sandbox::AgentStopReason::Owner)
        },
    ] {
        let mut changed = agent.clone();
        change(&mut changed);
        assert!(require_binding(&changed, &head, &receipt).is_err());
    }
}

#[test]
fn current_publication_refuses_receipt_and_head_mismatch() {
    let (agent, head, receipt) = fixture();
    for change in [
        |r: &mut Publication| r.head_revision += 1,
        |r: &mut Publication| r.last_seq += 1,
        |r: &mut Publication| r.result_revision += 1,
        |r: &mut Publication| r.incarnation = "b".repeat(64),
        |r: &mut Publication| r.active_ordinal += 1,
        |r: &mut Publication| r.agent_digest = Some(digest::binary(vec![1; 32])),
    ] {
        let mut changed = receipt.clone();
        change(&mut changed);
        assert!(require_binding(&agent, &head, &changed).is_err());
    }
    let mut changed = head.clone();
    changed.revision += 1;
    assert!(require_binding(&agent, &changed, &receipt).is_err());
}

#[test]
fn current_publication_reads_old_witnesses_without_promoting_them_to_current_proof() {
    let (agent, head, mut receipt) = fixture();
    receipt.format_version = 1;
    receipt.agent_digest = None;
    let row = bson::serialize_to_document(&receipt).unwrap();
    assert!(!row.contains_key("agent_digest"));
    Publication::read(row.clone()).unwrap();
    assert!(require_binding(&agent, &head, &receipt).is_err());
    let mut null = row;
    null.insert("agent_digest", bson::Bson::Null);
    assert!(Publication::read(null).is_err());
}

#[test]
fn current_publication_format_two_requires_witness_and_rejects_unknown_shape() {
    let (_, _, receipt) = fixture();
    let original = bson::serialize_to_document(&receipt).unwrap();
    for field in original.keys() {
        let mut changed = original.clone();
        changed.remove(field);
        assert!(Publication::read(changed).is_err(), "missing {field}");
    }
    for value in [
        bson::Bson::Null,
        bson::Bson::String("invalid".into()),
        bson::Bson::Binary(digest::binary(vec![1; 31])),
    ] {
        let mut changed = original.clone();
        changed.insert("agent_digest", value);
        assert!(Publication::read(changed).is_err());
    }
    let mut changed = original;
    changed.insert("format_version", 3_i64);
    assert!(Publication::read(changed).is_err());
}
