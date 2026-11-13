//! Persisted selection is checked before any Agent selection or replay.
use super::*;
use layer2_core::sandbox::AgentOrigin;
use layer2_core::trigger_domain::Launch;
use mongodb::bson::Document;

pub(in crate::store) async fn selected(
    store: &MongoSandboxStore,
    admission: &AgentAdmission,
    session: &mut mongodb::ClientSession,
) -> Result<Option<Launch>, ApplyError> {
    let AgentOrigin::Trigger { launch_id, .. } = &admission.origin else {
        return Ok(None);
    };
    let database = store.client.database(&store.agents.namespace().db);
    let launches =
        crate::store::durability::majority_collection::<Document>(&database, "trigger_launches");
    let document = launches
        .find_one(doc! {"_id":launch_id})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let receipts = crate::store::durability::majority_collection::<Document>(
        &database,
        "trigger_launch_input_protocols",
    );
    let receipt = receipts
        .find_one(doc! {"_id":launch_id})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let binding = crate::store::trigger_first_messages::collection(store)
        .find_one(doc! {"_id":launch_id})
        .session(&mut *session)
        .await
        .map_err(ApplyError::Driver)?;
    let Some(document) = document else {
        return if receipt.is_none() && binding.is_none() {
            Ok(None)
        } else {
            Err(ApplyError::Domain(SandboxStoreError::Conflict))
        };
    };
    let launch = crate::store::trigger_definitions::decode_launch_aggregate(&document)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    crate::store::trigger_definitions::require_input_protocol_document(&launch, receipt)
        .map_err(|error| ApplyError::Domain(SandboxStoreError::Corrupt(error.to_string())))?;
    crate::store::trigger_first_messages::require_launch_binding(&launch, binding.as_ref())
        .map_err(ApplyError::Domain)?;
    Ok(launch.input_protocol.is_some().then_some(launch))
}
