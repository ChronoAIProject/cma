use crate::sandbox::agent_io::*;
use crate::sandbox::store::{AgentLogStore, AgentStore};
use crate::sandbox::*;

/// Exercise the real dispatch fixtures against both recovery implementations.
/// The source selection represents indexed lookups; only the legacy side folds
/// every row, including unrelated work after the command's terminal event.
pub(super) async fn assert_equivalent(
    store: &InMemorySandboxStore,
    command: &AdmittedAgentCommand,
) {
    let dispatch = command.work.dispatch.as_ref().unwrap();
    let agent = store
        .get_agent(&dispatch.claim.agent_id)
        .await
        .unwrap()
        .unwrap();
    let entries = store.agent_log(&agent.id, 0, 200).await.unwrap();
    assert_eq!(entries.last().unwrap().seq, agent.conversation.last_seq);
    let mut legacy = CommandEvidence::new(command).unwrap();
    let mut pending = PendingAgentApprovals::default();
    for entry in &entries {
        legacy.observe(entry).unwrap();
        pending.observe(entry);
    }
    let get = |sequence| {
        entries
            .iter()
            .find(|entry| entry.seq == sequence)
            .unwrap()
            .clone()
    };
    let latest_delivery = entries.iter().rev().find(|entry|matches!(entry.kind,
        AgentLogKind::OperationDeliveryChanged { operation_sequence, .. } if operation_sequence == dispatch.operation_sequence)).unwrap().clone();
    let terminal = entries.iter().find(|entry|matches!(entry.kind,
        AgentLogKind::TurnCompleted { turn_sequence } | AgentLogKind::TurnFailed { turn_sequence, .. }
            | AgentLogKind::TurnInterrupted { turn_sequence } if turn_sequence == dispatch.turn_sequence)).cloned();
    let projected = CommandEvidence::from_projection(
        command,
        CommandLogReferences {
            through_seq: agent.conversation.last_seq,
            turn: get(dispatch.turn_sequence),
            fence: get(dispatch.operation_sequence),
            latest_delivery,
            terminal,
            has_pending_approvals: agent.conversation.active_turn == Some(dispatch.turn_sequence)
                && !pending.into_entries().is_empty(),
        },
    )
    .unwrap();
    assert_eq!(
        legacy.has_source_deferred(),
        projected.has_source_deferred()
    );
    let mut full_command = command.clone();
    let mut indexed_command = command.clone();
    let mut full_lane = CommandLane {
        lease: command.work.lease.clone(),
        ..Default::default()
    };
    let mut indexed_lane = full_lane.clone();
    let full_result = legacy.apply(&mut full_command, &mut full_lane, &agent);
    let indexed_result = projected.apply(&mut indexed_command, &mut indexed_lane, &agent);
    assert_eq!(full_result, indexed_result);
    assert_eq!(full_command, indexed_command);
    assert_eq!(full_lane, indexed_lane);
}
