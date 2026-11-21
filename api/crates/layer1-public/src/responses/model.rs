//! A response retains the model selected at its durable turn fence.
use layer2_core::sandbox::{AgentLogKind, AgentOperationKind};

pub(super) fn for_turn<'a>(
    initial: Option<&str>,
    turn: u64,
    history: impl IntoIterator<Item = (u64, Option<&'a AgentLogKind>)>,
) -> Option<String> {
    let mut legacy_known = true;
    for (seq, kind) in history {
        match kind {
            Some(AgentLogKind::OperationDeliveryChanged {
                operation: AgentOperationKind::UpdateSettings,
                ..
            }) => legacy_known = false,
            Some(AgentLogKind::TurnStarted {
                turn_sequence,
                model,
                ..
            }) if seq == turn && *turn_sequence == turn => {
                return Some(match model.as_deref() {
                    Some(model) => named_or_default(Some(model)),
                    None if legacy_known => named_or_default(initial),
                    // Old records did not retain settings values. Neither the
                    // immutable profile nor today's settings prove this turn.
                    None => "unknown".into(),
                });
            }
            _ => {}
        }
    }
    None
}

pub(super) fn named_or_default(model: Option<&str>) -> String {
    model
        .filter(|model| !model.is_empty())
        .unwrap_or("deployment-default")
        .into()
}
