use super::model::{Root, State};
use crate::{
    ingress::routing_store::{AdmittedPlan, StoredTarget},
    replicas::unresolved::ReleaseRecord,
    turns::routed::ExecutionError,
};

pub(crate) fn record(
    value: serde_json::Value,
    version: u64,
    plan: &AdmittedPlan,
    target: &StoredTarget,
    work: &str,
    operation: &str,
) -> Result<ReleaseRecord, ExecutionError> {
    let mut root: Root = serde_json::from_value(value).map_err(|_| ExecutionError::Corrupt)?;
    root.version = version;
    root.validate(plan, target)?;
    if root.id != work {
        return Err(ExecutionError::Corrupt);
    }
    let original_phase = serde_json::to_value(&root.state).map_err(|_| ExecutionError::Corrupt)?;
    let (phase, receipt, unresolved) = match &root.state {
        State::Sending => ("sending", None, true),
        State::Unknown { receipt } => ("unknown", receipt.clone(), true),
        State::UnknownAbandoned { receipt, .. } => ("unknown_abandoned", receipt.clone(), false),
        State::Queued => ("queued", None, false),
        State::Settled { receipt } => ("settled", Some(receipt.clone()), false),
    };
    root.state = State::UnknownAbandoned {
        receipt,
        operation_id: operation.into(),
    };
    Ok(ReleaseRecord {
        id: root.id.clone(),
        version,
        retained_until: root.retained_until,
        expires_at: root.expires_at,
        phase,
        unresolved,
        effect: true,
        original_phase,
        abandoned: serde_json::to_value(root).map_err(|_| ExecutionError::Corrupt)?,
    })
}
