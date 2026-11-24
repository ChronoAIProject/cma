use super::*;
use layer2_core::sandbox::AgentLogKind;

pub(super) async fn settle(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    mutation: &AgentWorkMutation,
    parents: Option<&AgentWorkPublicationParents>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    if !matches!(
        mutation.agent_logs.first(),
        Some(AgentLogKind::TurnInterrupted { .. })
    ) {
        return Ok(());
    }
    if parents.is_none()
        || !super::super::agent_control_receipt::has_owner_stop_receipt(row, current)
    {
        return Err(ApplyError::Domain(SandboxStoreError::Conflict));
    }
    super::super::budget::settle_owner_stop(store, current, &mutation.appended_at, session)
        .await
        .map_err(|error| match error {
            super::super::agent_conversation_store::ApplyError::Domain(error) => {
                ApplyError::Domain(error)
            }
            super::super::agent_conversation_store::ApplyError::Driver(error) => {
                ApplyError::Driver(error)
            }
        })
}
