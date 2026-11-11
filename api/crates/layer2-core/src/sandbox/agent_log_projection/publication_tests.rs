use super::*;
use crate::sandbox::{QueuedAgentMessage, QueuedMessageState};

const NOW: &str = "2026-11-11T00:00:00Z";

fn batch(agent: &Agent, kinds: Vec<AgentLogKind>) -> Vec<AgentLogEntry> {
    kinds
        .into_iter()
        .enumerate()
        .map(|(index, kind)| AgentLogEntry {
            agent_id: agent.id.clone(),
            seq: agent.conversation.last_seq + index as u64 + 1,
            appended_at: NOW.into(),
            kind,
        })
        .collect()
}

fn turn(agent: &Agent) -> (Agent, Vec<AgentLogEntry>) {
    let sequence = agent.conversation.last_seq + 1;
    let entries = batch(
        agent,
        vec![
            AgentLogKind::TurnStarted {
                turn_sequence: sequence,
                origin_key: Some("original-command".into()),
                model: Some("original-model".into()),
            },
            AgentLogKind::OwnerMessage {
                content: "original input".into(),
            },
        ],
    );
    let mut next = agent.clone();
    next.revision += 1;
    next.conversation.last_seq += entries.len() as u64;
    next.conversation.active_turn = Some(sequence);
    (next, entries)
}

#[test]
fn whole_publication_is_deterministic_and_returns_only_complete_semantics() {
    let (current, source) = super::super::tests::opened();
    let original = source.clone();
    let (next, entries) = turn(&current);
    let first = source.plan_publication(&current, &next, &entries).unwrap();
    let retry = source.plan_publication(&current, &next, &entries).unwrap();
    assert_eq!(first, retry);
    assert_eq!(source, original);
    let facts = first.0.active.as_ref().unwrap();
    assert_eq!(facts.owner_sequence, Some(4));
    assert_eq!(facts.origin_key.as_deref(), Some("original-command"));
    assert_eq!(facts.model.as_deref(), Some("original-model"));
    assert_eq!(first.0.through_seq, 4);

    let mut incomplete = next.clone();
    incomplete.conversation.last_seq -= 1;
    assert!(source
        .plan_publication(&current, &incomplete, &entries[..1])
        .is_err());
    assert_eq!(source, original);
}

#[test]
fn publication_rejects_changed_lineage_stale_source_and_mixed_retry_time() {
    let (current, source) = super::super::tests::opened();
    let (next, entries) = turn(&current);
    for alter in [
        |agent: &mut Agent| agent.id.0.push('x'),
        |agent: &mut Agent| agent.creation_key.push('x'),
        |agent: &mut Agent| agent.creation_fingerprint.push('x'),
        |agent: &mut Agent| agent.created_at = "2026-11-11T01:00:00Z".into(),
        |agent: &mut Agent| agent.workspace_id.0.push('x'),
        |agent: &mut Agent| agent.sandbox_id.0.push('x'),
        |agent: &mut Agent| agent.worktree_id.0.push('x'),
        |agent: &mut Agent| agent.owner = crate::SubjectId::new("other-fixture-owner"),
        |agent: &mut Agent| agent.revision = 0,
        |agent: &mut Agent| agent.conversation.last_seq += 1,
    ] {
        let mut changed = next.clone();
        alter(&mut changed);
        assert!(source
            .plan_publication(&current, &changed, &entries)
            .is_err());
    }
    let mut stale = source.clone();
    stale.through_seq -= 1;
    assert!(stale.plan_publication(&current, &next, &entries).is_err());
    let mut changed = entries.clone();
    changed[1].appended_at = "2026-11-11T01:00:00Z".into();
    assert!(source.plan_publication(&current, &next, &changed).is_err());
    changed = entries.clone();
    changed[0].seq += 1;
    assert!(source.plan_publication(&current, &next, &changed).is_err());
    changed = entries;
    changed[0].agent_id.0.push('x');
    assert!(source.plan_publication(&current, &next, &changed).is_err());
}

#[test]
fn bounded_batches_keep_global_sequences_across_physical_segment_boundaries() {
    let (mut current, mut source) = super::super::tests::opened();
    current.conversation.last_seq = 10_000;
    source.through_seq = 10_000;
    let entries = batch(
        &current,
        vec![AgentLogKind::ConversationGap { frames: 1 }; 64],
    );
    let mut next = current.clone();
    next.conversation.last_seq += 64;
    let (planned, changes) = source.plan_publication(&current, &next, &entries).unwrap();
    assert_eq!(planned.through_seq, 10_064);
    assert!(changes.is_empty());
    assert_eq!(source.through_seq, 10_000);
    let mut too_many = entries;
    too_many.push(AgentLogEntry {
        agent_id: current.id.clone(),
        seq: 10_065,
        appended_at: NOW.into(),
        kind: AgentLogKind::ConversationGap { frames: 1 },
    });
    next.conversation.last_seq += 1;
    assert!(source.plan_publication(&current, &next, &too_many).is_err());
}

#[test]
fn metadata_only_publication_keeps_source_and_does_not_invent_a_native_open() {
    let (current, source) = super::super::tests::opened();
    let mut next = current.clone();
    next.revision += 1;
    let (planned, changes) = source.plan_publication(&current, &next, &[]).unwrap();
    assert_eq!(planned, source);
    assert!(changes.is_empty());
    next.conversation.runtime_generation = Some(2);
    assert!(source.plan_publication(&current, &next, &[]).is_err());
}

#[test]
fn queued_publication_consumes_one_exact_source_and_cannot_reuse_it() {
    let (mut current, mut source) = super::super::tests::opened();
    let queued = QueuedAgentMessage::new(3, "original queued input".into(), NOW.into()).unwrap();
    let entry = batch(
        &current,
        vec![AgentLogKind::OwnerMessage {
            content: queued.content.clone(),
        }],
    );
    current.conversation.last_seq = 3;
    current.conversation.queued_messages.push(queued);
    source.observe(&current, &entry[0], None).unwrap();
    source.validate_aggregate(&current).unwrap();
    let entries = batch(
        &current,
        vec![
            AgentLogKind::TurnStarted {
                turn_sequence: 4,
                origin_key: Some("queued-message:3".into()),
                model: None,
            },
            AgentLogKind::TurnCompleted { turn_sequence: 4 },
        ],
    );
    let mut next = current.clone();
    next.conversation.last_seq += 2;
    next.conversation.queued_messages[0].state = QueuedMessageState::Delivered;
    next.conversation.queued_messages[0].dispatch_fenced_at = Some(NOW.into());
    let (planned, _) = source.plan_publication(&current, &next, &entries).unwrap();
    assert_eq!(
        planned.latest_completed.as_ref().unwrap().owner_sequence,
        Some(3)
    );
    let repeated = batch(
        &next,
        vec![AgentLogKind::TurnStarted {
            turn_sequence: 6,
            origin_key: Some("queued-message:3".into()),
            model: None,
        }],
    );
    let mut repeated_next = next.clone();
    repeated_next.conversation.last_seq += 1;
    repeated_next.conversation.active_turn = Some(6);
    assert!(planned
        .plan_publication(&next, &repeated_next, &repeated)
        .is_err());
}
