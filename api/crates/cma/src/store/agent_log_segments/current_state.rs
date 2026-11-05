//! Bounded current controls, independent of a displayed history window.
use super::{
    approval_pages, approvals::store_error, consumers::points, corrupt, projection,
    projection_reads::snapshot,
};
use crate::store::sandbox_mongo::MongoSandboxStore;
use layer2_core::sandbox::{
    store::{AgentLogCurrentState, SandboxStoreError},
    Agent, AgentLogKind,
};

pub(in crate::store) async fn read(
    store: &MongoSandboxStore,
    agent: &Agent,
    after: u64,
    limit: u16,
) -> Result<Option<AgentLogCurrentState>, SandboxStoreError> {
    let Some((head, mut session)) = snapshot(store, agent).await? else {
        return Ok(None);
    };
    let state = projection::read(store, agent, &head, &mut session)
        .await
        .map_err(store_error)?;
    let mut references = [
        agent.conversation.active_turn,
        state
            .active
            .as_ref()
            .and_then(|turn| turn.first_gap_sequence),
        state.latest_commands,
        state.latest_plan,
        state.latest_metadata,
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>();
    references.sort_unstable();
    let context = points(store, &head, &references, &mut session, true)
        .await
        .map_err(store_error)?;
    for entry in &context {
        let expected = match &entry.kind {
            AgentLogKind::TurnStarted { turn_sequence, .. } if *turn_sequence == entry.seq => {
                agent.conversation.active_turn
            }
            AgentLogKind::ConversationGap { frames } if *frames > 0 => state
                .active
                .as_ref()
                .and_then(|turn| turn.first_gap_sequence),
            AgentLogKind::AvailableCommandsSet { .. } => state.latest_commands,
            AgentLogKind::PlanSet { .. } => state.latest_plan,
            AgentLogKind::RuntimeMetadataChanged { .. } => state.latest_metadata,
            _ => return Err(corrupt()),
        };
        if expected != Some(entry.seq) {
            return Err(corrupt());
        }
    }
    let approvals = approval_pages::page(store, agent, after, limit, &mut session)
        .await
        .map_err(store_error)?
        .ok_or_else(corrupt)?;
    Ok(Some(AgentLogCurrentState { context, approvals }))
}
