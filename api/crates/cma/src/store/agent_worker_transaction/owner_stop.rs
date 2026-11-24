use super::*;
use layer2_core::sandbox::AgentLogKind;

fn interrupts_turn(mutation: &AgentWorkMutation) -> bool {
    matches!(
        mutation.agent_logs.first(),
        Some(AgentLogKind::TurnInterrupted { .. })
    )
}

pub(super) fn fence_live_lease(filter: &mut Document, mutation: &AgentWorkMutation) {
    if interrupts_turn(mutation) {
        filter.insert(
            "$expr",
            doc! {
                "$gt": [
                    { "$dateFromString": {
                        "dateString": "$work_lease_expires_at", "onError": null, "onNull": null,
                    }},
                    "$$NOW",
                ],
            },
        );
    }
}

pub(super) async fn settle(
    store: &MongoSandboxStore,
    row: &Document,
    current: &Agent,
    mutation: &AgentWorkMutation,
    parents: Option<&AgentWorkPublicationParents>,
    session: &mut mongodb::ClientSession,
) -> Result<(), ApplyError> {
    if !interrupts_turn(mutation) {
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
