use super::*;

/// Grant, current retained creation identity and Bridge transition commit in one
/// snapshot. Resource handles are installed when invoke storage is composed;
/// missing composition fails closed, without an Arc cycle back to definitions.
pub(in crate::store) async fn fence_enable(
    bridges: &MongoBridgeStore,
    proof: &BridgeInvokeEnableProof,
    current: &BridgeRecord,
    session: &mut ClientSession,
) -> Result<(), BridgeError> {
    proof.require_current()?;
    let resources = bridges
        .invoke_resources
        .get()
        .ok_or(BridgeError::Unavailable)?;
    let database = bridges.client.database(&bridges.definitions.namespace().db);
    let grants = majority_collection::<Document>(&database, "bridge_invoke_grants");
    let document = grants
        .find_one(doc! {"_id":proof.grant().id.as_str()})
        .session(&mut *session)
        .await
        .map_err(|_| BridgeError::Unavailable)?
        .ok_or(BridgeError::ConnectionRequired(
            layer2_core::bridge::BridgeSetupBlock::InvokeConsentRequired,
        ))?;
    let grant = read_grant(&document).map_err(|_| BridgeError::Corrupt)?;
    let snapshot = read_execution_snapshot(resources, &grant.target.resource(), session)
        .await
        .map_err(resource_error)?;
    proof.require_snapshot(current, &grant, &snapshot)?;
    let changed = grants
        .update_one(
            doc! {"_id":grant.id.as_str(),"revision":grant.revision as i64},
            doc! {"$inc":{"enable_fence":1_i64}},
        )
        .session(&mut *session)
        .await
        .map_err(|_| BridgeError::Unavailable)?;
    if changed.matched_count != 1 {
        return Err(BridgeError::Conflict);
    }
    fence_lineage(resources, &snapshot, session)
        .await
        .map_err(resource_error)?;
    proof.require_current()
}
fn resource_error(error: ReadError) -> BridgeError {
    match error {
        ReadError::Driver(_) => BridgeError::Unavailable,
        ReadError::Domain(layer2_core::resource_access::ResourceAccessStoreError::Unavailable) => {
            BridgeError::Unavailable
        }
        ReadError::Domain(_) => {
            BridgeError::ConnectionRequired(layer2_core::bridge::BridgeSetupBlock::TargetMismatch)
        }
    }
}
